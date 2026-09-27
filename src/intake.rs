//! Приём снимков с телефона напрямую, без ВК.
//!
//! Программа поднимает у себя в локальной сети маленький сервер и показывает
//! QR-код. Врач наводит камеру — открывается страница, выбирает снятые
//! страницы, и они приходят прямо в программу.

use std::io::{Cursor, Read};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use qrcode::QrCode;
use tiny_http::{Header, Method, Request, Response, Server};

/// Больше этого за одну отправку не принимаем: снимок страницы с айфона — 3–6 МБ,
/// сорока хватает на десяток, а память программы остаётся под контролем.
const MAX_BODY: usize = 40 * 1024 * 1024;

/// Пришедший с телефона файл.
#[derive(Debug, Clone)]
pub struct Received {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Запасной путь: он работает всегда и его надо держать на виду в окне, а не
/// показывать только после неудачи.
pub const FALLBACK: &str =
    "Если снимки не идут: включите на телефоне «Режим модема», подключите к этой раздаче компьютер \
и нажмите «Проверить приём» — тогда телефон и компьютер точно в одной сети. \
И в любую минуту снимки можно просто перетащить в окно программы мышкой.";

/// Порт приёма фиксирован, а не случаен.
///
/// Разрешение в брандмауэре выдаётся на НОМЕР порта. Со случайным портом такое
/// разрешение действовало бы ровно один запуск программы, и врач ходила бы к
/// админу каждое утро. Занят — берём любой свободный, но это уже редкий случай.
const PORT: u16 = 40445;

/// Что показать врачу по итогам самопроверки: что случилось, почему и что
/// нажать. Отдельно — команда для админа, если причина в брандмауэре.
#[derive(Debug, Clone)]
pub struct Checkup {
    /// Отвечает ли сервер по тому адресу, который сейчас в QR-коде.
    pub ok: bool,
    /// Полный текст для врача: заголовок, причина и шаги.
    pub text: String,
    /// Ровно одна команда, которую вводит админ. Программа её не выполняет:
    /// прав нет, машина в домене и чужая.
    pub command: Option<String>,
    /// Одна строка о том, что команда делает.
    pub command_note: Option<String>,
}

pub struct Intake {
    ip: IpAddr,
    port: u16,
    token: String,
    url: String,
    ifaces: Vec<(String, IpAddr)>,
    last: Option<Checkup>,
    server: Arc<Server>,
    files: Arc<Mutex<Vec<Received>>>,
    count: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl Intake {
    /// Поднять сервер на свободном порту в локальной сети.
    pub fn start() -> Result<Intake> {
        let mut secret = [0u8; 16];
        getrandom::fill(&mut secret)
            .map_err(|e| anyhow!("не создать код доступа к приёму снимков: {e}"))?;
        let token: String = secret.iter().map(|b| format!("{b:02x}")).collect();
        let ifaces = interfaces();
        let ip = ifaces
            .first()
            .map(|(_, ip)| *ip)
            .ok_or_else(|| anyhow!("нет подключения к сети — перетащите снимки мышкой"))?;

        // Слушаем все интерфейсы, а не один адрес. На рабочей машине их обычно
        // два: проводная сеть поликлиники и Wi-Fi, раздаваемый с телефона (это
        // и советует подсказка в окне, когда сеть клиники телефон не пускает).
        // Привязка к одному адресу означала, что по второму никто не отвечает,
        // и обходной путь через раздачу не работал вовсе.
        let server = Server::http(("0.0.0.0", PORT))
            .or_else(|_| Server::http(("0.0.0.0", 0)))
            .map_err(|e| anyhow!("не удалось открыть приём снимков: {e}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| anyhow!("не удалось открыть приём снимков"))?
            .port();

        let server = Arc::new(server);
        let files: Arc<Mutex<Vec<Received>>> = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let hits = Arc::new(AtomicUsize::new(0));

        let worker = {
            let (server, files, count, hits) =
                (server.clone(), files.clone(), count.clone(), hits.clone());
            let token = token.clone();
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    handle(req, &files, &count, &hits, &token);
                }
            })
        };

        crate::note(&format!(
            "приём: сервер слушает 0.0.0.0:{port} (все сети машины)"
        ));
        log_ifaces(&ifaces);
        let url = url_of(ip, port, &token);
        crate::note(&format!("приём: QR-код для {ip}:{port} готов"));

        Ok(Intake {
            ip,
            port,
            token,
            url,
            ifaces,
            last: None,
            server,
            files,
            count,
            hits,
            worker: Some(worker),
        })
    }

    /// Адрес приёма с одноразовым кодом доступа — его и кодируем в QR.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Все адреса машины с именами сетей — из них врач выбирает, по какому
    /// строить QR. Первым идёт раздача с телефона, если она поднята.
    pub fn addresses(&self) -> &[(String, IpAddr)] {
        &self.ifaces
    }

    /// Адрес, который сейчас в QR-коде.
    pub fn ip(&self) -> IpAddr {
        self.ip
    }

    /// Переключить QR на другой адрес машины. Сервер перезапускать не надо:
    /// он и так слушает все сети сразу.
    pub fn use_address(&mut self, ip: IpAddr) {
        if ip == self.ip {
            return;
        }
        self.ip = ip;
        self.url = url_of(ip, self.port, &self.token);
        self.last = None;
        crate::note(&format!("приём: QR-код для {ip}:{} готов", self.port));
    }

    /// Сколько раз до нас достучались снаружи — не считая собственной
    /// самопроверки.
    ///
    /// Это единственное, чем «сервер отвечает мне самому» отличается от «до
    /// меня можно достучаться снаружи». Своим же адресом машина ходит сама к
    /// себе через петлю, INPUT-правила внешнего интерфейса при этом не
    /// срабатывают — поэтому при наглухо закрытом брандмауэре самопроверка
    /// всё равно скажет «отвечает». А ноль обращений при живом сервере — это
    /// уже факт: ни один пакет с телефона до нас не долетел.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    /// Самопроверка: программа сама открывает свой же адрес — ровно так же,
    /// как это сделал бы телефон.
    ///
    /// Отделяет «сервер не поднялся» от «телефон не доходит»: у врача оба
    /// случая выглядят одинаково — белым листом в Safari.
    pub fn check(&mut self) {
        // Список сетей мог измениться с момента запуска: раздачу с телефона
        // врач включает уже после того, как код показан.
        let now = interfaces();
        if now != self.ifaces {
            log_ifaces(&now);
            let gone = !now.iter().any(|(_, ip)| *ip == self.ip);
            let first = now.first().map(|(_, ip)| *ip);
            self.ifaces = now;
            // Сеть, по которой строился код, исчезла — код стал мусором.
            if let (true, Some(ip)) = (gone, first) {
                self.use_address(ip);
            }
        }

        // Две разные проверки, и в этом весь смысл.
        //
        // Петля отвечает на то, поднялся ли сервер вообще. Адрес из QR-кода
        // отвечает на то, обслуживается ли ИМЕННО он: у VPN и виртуальных
        // адаптеров своя маршрутизация, и до их адреса не доходит даже сама
        // машина. Ни одна из двух не доказывает, что доходит телефон, — это
        // доказывают только обращения в счётчике.
        let state = if self.ifaces.is_empty() {
            Reach::NoNetwork
        } else if let Err(e) = probe(
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), self.port),
            &self.token,
        ) {
            Reach::ServerDown(e)
        } else if let Err(e) = probe(SocketAddr::new(self.ip, self.port), &self.token) {
            Reach::AddressUnreachable(e)
        } else {
            Reach::Serving
        };

        crate::note(&format!(
            "приём: самопроверка {}:{} — {state:?}, обращений снаружи {}, снимков {}",
            self.ip,
            self.port,
            self.hits(),
            self.count()
        ));
        self.last = Some(diagnose(
            state,
            self.ip,
            self.port,
            &self.url,
            self.hits(),
            self.count(),
        ));
    }

    /// Итог последней самопроверки: удалось ли и что показать врачу.
    pub fn last_check(&self) -> Option<(bool, &str)> {
        self.last.as_ref().map(|c| (c.ok, c.text.as_str()))
    }

    /// То же, но целиком: с командой для админа, когда причина в брандмауэре.
    pub fn last_checkup(&self) -> Option<&Checkup> {
        self.last.as_ref()
    }

    /// QR-код этого адреса как PNG, чтобы показать в окне.
    pub fn qr_png(&self) -> Result<Vec<u8>> {
        qr_png_for(&self.url)
    }

    /// Забрать всё, что пришло с прошлого вызова. Не блокирует.
    pub fn take(&self) -> Vec<Received> {
        std::mem::take(&mut *lock(&self.files))
    }

    /// Сколько файлов принято всего — для строки «принято: 3».
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

impl Drop for Intake {
    fn drop(&mut self) {
        crate::note(&format!(
            "приём: выключен, снимков принято всего {}",
            self.count()
        ));
        // Без этого поток так и стоит в ожидании запроса и держит закрытие окна.
        self.server.unblock();
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn url_of(ip: IpAddr, port: u16, token: &str) -> String {
    // SocketAddr сам расставит скобки, если адрес окажется IPv6.
    format!("http://{}/{token}/", SocketAddr::new(ip, port))
}

/// Все адреса машины, по которым телефон в принципе может постучаться.
///
/// Первой идёт раздача Wi-Fi с айфона (она всегда 172.20.10.x): проводная сеть
/// поликлиники телефон обычно к компьютеру не пускает, а раздача — это тот
/// самый обходной путь. Петля отброшена по адресу и по имени интерфейса
/// `lo`: в WSL у неё бывает обычный на вид адрес 10.255.255.254.
// ponytail: узнаём только раздачу айфона; для андроида (192.168.43.x) здесь
// добавится ещё одна ветка, когда такой телефон появится.
pub fn interfaces() -> Vec<(String, IpAddr)> {
    let mut v: Vec<(String, IpAddr)> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, ip)| {
            name != "lo"
                && ip.is_ipv4()
                && !ip.is_loopback()
                && !ip.is_unspecified()
                && !is_dead_link(ip)
        })
        .collect();
    // Сортировка устойчивая: раздача наверх, остальное — в порядке системы.
    v.sort_by_key(|(_, ip)| !is_hotspot(ip));
    if v.is_empty() {
        // Списка интерфейсов может не быть (урезанная система, отказ вызова) —
        // тогда спрашиваем хотя бы основной адрес.
        if let Ok(ip) = local_ip_address::local_ip() {
            if !ip.is_loopback() {
                v.push(("сеть".to_string(), ip));
            }
        }
    }
    v
}

/// Адрес из раздачи Wi-Fi с айфона — его в списке помечаем словами, а не
/// цифрами: «172.20.10.3» врачу ничего не говорит.
pub fn is_hotspot(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[..3] == [172, 20, 10])
}

/// Адрес отключённого адаптера: 169.254.0.0/16 машина назначает себе сама,
/// когда сеть есть на бумаге, а связи нет (кабель выдернут, виртуальный
/// адаптер простаивает).
///
/// В список выбора такие пускать нельзя: врач ткнёт в адрес, телефон до него
/// не дойдёт никогда, и виноватой окажется программа. Поймано на живой
/// машине — система на попытку соединиться отвечает прямым текстом:
/// «Сделана попытка выполнить операцию на сокете при отключенной сети».
fn is_dead_link(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[..2] == [169, 254])
}

fn log_ifaces(list: &[(String, IpAddr)]) {
    if list.is_empty() {
        crate::note("приём: сетевых адресов у машины не найдено");
        return;
    }
    let s: Vec<String> = list.iter().map(|(n, ip)| format!("{n} {ip}")).collect();
    crate::note(&format!("приём: сети машины: {}", s.join(", ")));
}

/// Сходить на собственный адрес ровно так, как сходил бы телефон: обычный
/// GET и обычная страница в ответ.
///
/// Отдельная функция, а не метод, чтобы проверкой можно было ткнуть и в
/// заглушённый сервер — иначе не доказать, что самопроверка вообще умеет
/// говорить «нет».
pub fn probe(addr: SocketAddr, token: &str) -> std::result::Result<(), String> {
    use std::io::Write;
    // Три секунды: свой же адрес отвечает мгновенно, а врач ждёт у экрана.
    const WAIT: std::time::Duration = std::time::Duration::from_secs(3);

    let mut s = std::net::TcpStream::connect_timeout(&addr, WAIT)
        .map_err(|e| format!("соединение не открылось ({e})"))?;
    let _ = s.set_read_timeout(Some(WAIT));
    let _ = s.set_write_timeout(Some(WAIT));
    // Метка в адресе: сервер по ней узнаёт собственную самопроверку и не
    // засчитывает её как обращение с телефона. Без метки счётчик обращений
    // накручивала бы сама кнопка «Проверить приём», и главный признак —
    // «снаружи не пришёл никто» — перестал бы работать.
    write!(
        s,
        "GET /{token}/{SELF_CHECK} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("запрос не ушёл ({e})"))?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = s.read(&mut chunk) {
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
        // tiny_http держит соединение и после ответа — ждём конец разметки,
        // а не конец потока, иначе упрёмся в таймаут на каждой проверке.
        if raw.ends_with(b"</html>") || raw.len() > 64 * 1024 {
            break;
        }
    }
    if raw.is_empty() {
        return Err("ответа нет".to_string());
    }
    let text = String::from_utf8_lossy(&raw);
    if !text.starts_with("HTTP/1.1 200") {
        let first = text.lines().next().unwrap_or("").trim().to_string();
        return Err(format!("ответ не тот: {first}"));
    }
    if !text.contains("Снимки в программу") {
        return Err("страница пришла пустой".to_string());
    }
    Ok(())
}

// ── что именно не так ───────────────────────────────────────────────────────

/// Адрес, по которому программа стучится сама к себе. Обычная страница в
/// ответ, но в журнале и в счётчике обращений такой запрос отделён от
/// настоящего телефона. Метка латиницей: кириллица в строке запроса потребовала
/// бы кодирования, а ошибиться тут нельзя — на метке держится весь разбор.
const SELF_CHECK: &str = "?self-check";

/// До чего программа дозвонилась сама. Ни один вариант не доказывает, что
/// доходит телефон, — этим занимается счётчик обращений.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Сетевых адресов у машины нет вовсе.
    NoNetwork,
    /// Не отвечает даже петля: сервер не поднялся или уже умер.
    ServerDown(String),
    /// Сервер жив, но конкретно этот адрес не обслуживается (VPN, виртуальный
    /// адаптер со своей маршрутизацией).
    AddressUnreachable(String),
    /// Сервер жив и адрес из QR-кода обслуживается.
    Serving,
}

/// Весь разбор — в одной чистой функции: сети она не трогает, поэтому каждый
/// исход проверяется тестом, а не живой поликлиникой.
pub fn diagnose(
    state: Reach,
    ip: IpAddr,
    port: u16,
    url: &str,
    hits: usize,
    files: usize,
) -> Checkup {
    let plain = |text: String, ok: bool| Checkup {
        ok,
        text,
        command: None,
        command_note: None,
    };

    match state {
        Reach::NoNetwork => plain(
            "Компьютер не подключён ни к одной сети.\n\n\
             Пока сети нет, телефон до программы не дойдёт, какой бы код ни был показан.\n\n\
             Что делать: включите на телефоне «Режим модема», подключите компьютер к этой \
             раздаче и нажмите «Проверить приём» — код перестроится сам. \
             Или перетащите снимки в окно программы мышкой."
                .to_string(),
            false,
        ),

        Reach::ServerDown(why) => plain(
            format!(
                "Приём снимков не запустился ({why}).\n\n\
                 Принимать снимки сейчас некуда, поэтому и код не работает.\n\n\
                 Что делать: закройте программу и откройте заново. Если не помогло — \
                 перетащите снимки в окно программы мышкой, этот путь работает всегда."
            ),
            false,
        ),

        Reach::AddressUnreachable(why) => plain(
            format!(
                "По адресу {url} компьютер не отвечает даже сам себе ({why}).\n\n\
                 Значит, в списке выбрана служебная сеть — например, VPN или виртуальный \
                 адаптер. Телефон к такой сети не подключён и никогда не подключится.\n\n\
                 Что делать: выберите в списке сетей другой адрес — тот, начало которого \
                 совпадает с сетью, к которой подключён телефон. Если не знаете какой — \
                 включите на телефоне «Режим модема» и подключите к нему компьютер: \
                 нужная сеть встанет в списке первой."
            ),
            false,
        ),

        // Сервер жив, адрес обслуживается — и при этом снаружи не пришёл никто.
        // Отличить брандмауэр от изоляции клиентов на точке доступа с самой
        // машины нельзя: обе причины выглядят одинаково — тишиной. Поэтому
        // называем обе и даём шаг, который закрывает каждую.
        Reach::Serving if hits == 0 => {
            let (command, note) = open_port_command(port);
            Checkup {
                ok: true,
                text: format!(
                    "Компьютер к приёму готов, но с телефона не пришло ни одного обращения.\n\n\
                     Программа работает и страницу отдаёт — её только что открыл сам компьютер. \
                     Значит, дело не в программе: пакеты с телефона до компьютера не долетают. \
                     Так бывает по двум причинам — либо сеть поликлиники не пропускает телефоны \
                     к компьютерам, либо на компьютере закрыт порт приёма.\n\n\
                     Что делать по порядку:\n\
                     1. Проверьте на телефоне Wi-Fi: он должен быть подключён к сети, а не к \
                     мобильному интернету. Адрес компьютера — {ip}, телефон должен быть в той же \
                     сети (первые числа адреса — {}).\n\
                     2. Если телефон в той же сети, а снимки всё равно не идут — покажите \
                     системному администратору команду ниже. Её вводят один раз.\n\
                     3. Обход, который работает всегда: включите на телефоне «Режим модема», \
                     подключите компьютер к этой раздаче и нажмите «Проверить приём» — код \
                     перестроится сам, и телефон с компьютером окажутся в одной сети.\n\
                     4. И в любом случае снимки можно перетащить в окно программы мышкой.",
                    network_hint(ip)
                ),
                command: Some(command),
                command_note: Some(note),
            }
        }

        Reach::Serving if files == 0 => plain(
            format!(
                "Телефон до компьютера дошёл: обращений — {hits}, а снимков пока ноль.\n\n\
                 Значит, сеть и брандмауэр ни при чём: страница на телефоне открывается, \
                 отправки просто ещё не было.\n\n\
                 Что делать: на телефоне нажмите «Выбрать снимки», отметьте снятые страницы \
                 и нажмите «Отправить»."
            ),
            true,
        ),

        Reach::Serving => plain(
            format!(
                "Приём работает: снимков принято {files}.\n\n\
                 Телефон до компьютера доходит, снимки приходят в программу."
            ),
            true,
        ),
    }
}

/// Первые два числа адреса — по ним врач сличает сеть компьютера с той, что
/// показывает телефон. Полный адрес для этого не нужен и только путает.
fn network_hint(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}", o[0], o[1])
        }
        IpAddr::V6(_) => ip.to_string(),
    }
}

/// Команда, которой порт приёма открывают один раз, и строка о том, что она
/// делает.
///
/// Сама программа правил не трогает: прав у неё нет, машина в домене и чужая,
/// а молча менять брандмауэр на рабочем месте в поликлинике нельзя.
///
/// На «Альт Рабочая станция 10» входящие на внешних интерфейсах по умолчанию
/// закрыты (Центр управления системой → «Брандмауэр» → «Внешние сети», модуль
/// alterator-net-iptables), поэтому высокий порт и не отвечает при живом
/// сервере. На части машин вместо этого поднят firewalld с зоной public, где
/// снаружи разрешены только SSH и DHCPv6 — команда там другая, выбираем по
/// тому, что реально стоит на машине.
pub fn open_port_command(port: u16) -> (String, String) {
    if cfg!(windows) {
        return (
            format!(
                "netsh advfirewall firewall add rule name=\"МеДок: приём снимков\" \
                 dir=in action=allow protocol=TCP localport={port}"
            ),
            format!(
                "Разрешает телефону подключаться к компьютеру по порту {port} — только по нему."
            ),
        );
    }
    if std::path::Path::new("/usr/bin/firewall-cmd").exists() {
        return (
            format!(
                "sudo firewall-cmd --permanent --add-port={port}/tcp && sudo firewall-cmd --reload"
            ),
            format!("Разрешает телефону подключаться к компьютеру по порту {port} и запоминает это после перезагрузки."),
        );
    }
    (
        format!("sudo iptables -I INPUT -p tcp --dport {port} -j ACCEPT && sudo service iptables save"),
        format!("Разрешает телефону подключаться к компьютеру по порту {port} и сохраняет разрешение, чтобы оно не пропало после перезагрузки."),
    )
}

/// Отравленный мьютекс не повод терять снимки: забираем данные как есть.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ── сервер ──────────────────────────────────────────────────────────────────

fn handle(
    mut req: Request,
    files: &Mutex<Vec<Received>>,
    count: &AtomicUsize,
    hits: &AtomicUsize,
    token: &str,
) {
    if req.url().split('?').next() != Some(format!("/{token}/").as_str()) {
        let _ = req.respond(html(
            404,
            page_error("Адрес приёма неверен. Откройте QR-код заново."),
        ));
        return;
    }
    // Самая нужная строчка в журнале: если она есть — телефон до компьютера
    // дошёл, и белая страница означает что угодно, только не «сеть не пускает».
    // Если её нет вовсе — до нас не долетело ни одного пакета.
    let own = req.url().contains(SELF_CHECK.trim_start_matches('?'));
    if !own {
        hits.fetch_add(1, Ordering::Relaxed);
    }
    crate::note(&format!(
        "приём: запрос {} с {}{}",
        req.method(),
        req.remote_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "неизвестно откуда".to_string()),
        if own {
            " (своя самопроверка)"
        } else {
            ""
        }
    ));
    let resp = if *req.method() == Method::Post {
        match accept_upload(&mut req, files, count) {
            Ok((ok, bad)) => html(200, page_done(ok, &bad)),
            Err((code, msg)) => html(code, page_error(&msg)),
        }
    } else {
        // На любой GET отдаём ту же страницу: адрес врач набирать не будет,
        // а лишний 404 в чужом браузере только пугает.
        html(200, page_form())
    };
    let _ = req.respond(resp);
}

/// Возвращает (принято, отклонённые имена) либо (код ответа, текст для врача).
fn accept_upload(
    req: &mut Request,
    files: &Mutex<Vec<Received>>,
    count: &AtomicUsize,
) -> std::result::Result<(usize, Vec<String>), (u16, String)> {
    let ct = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let boundary =
        match boundary_of(&ct) {
            Some(b) => b,
            None => return Err((
                400,
                "Телефон отправил снимки в непонятном виде. Откройте страницу заново и повторите."
                    .to_string(),
            )),
        };

    let body = match read_body(req.as_reader(), MAX_BODY) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Err((
                413,
                format!(
                    "Слишком много за один раз — больше {} МБ. Отправьте снимки двумя частями.",
                    MAX_BODY / 1024 / 1024
                ),
            ))
        }
        Err(_) => {
            return Err((
                400,
                "Связь с телефоном прервалась. Попробуйте отправить ещё раз.".to_string(),
            ))
        }
    };

    let parts = match parse_multipart(&body, &boundary) {
        Ok(p) => p,
        Err(_) => {
            return Err((
                400,
                "Не удалось разобрать отправку. Попробуйте ещё раз.".to_string(),
            ))
        }
    };

    let (mut good, mut bad) = (Vec::new(), Vec::new());
    for p in parts {
        if is_media(&p.name, &p.bytes) {
            good.push(p);
        } else {
            bad.push(p.name);
        }
    }
    let n = good.len();
    let total = count.fetch_add(n, Ordering::Relaxed) + n;
    lock(files).extend(good);
    // Имён файлов в журнал не пишем: имя снимка врач даёт по фамилии ребёнка.
    crate::note(&format!(
        "приём: принято снимков {n}, отклонено {}, всего {total}",
        bad.len()
    ));
    Ok((n, bad))
}

/// Читаем на байт больше лимита: пришёл — значит, отправка великовата.
fn read_body(r: impl Read, limit: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    r.take(limit as u64 + 1).read_to_end(&mut buf)?;
    Ok(if buf.len() > limit { None } else { Some(buf) })
}

fn html(status: u16, body: String) -> Response<Cursor<Vec<u8>>> {
    let mut r = Response::from_data(body.into_bytes()).with_status_code(status);
    // from_bytes здесь не может не сработать, но паниковать в потоке сервера
    // нельзя даже теоретически — молча обходимся без заголовка.
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]) {
        r.add_header(h);
    }
    r
}

// ── multipart ───────────────────────────────────────────────────────────────

fn boundary_of(content_type: &str) -> Option<String> {
    let low = content_type.to_ascii_lowercase();
    if !low.starts_with("multipart/form-data") {
        return None;
    }
    let at = low.find("boundary=")? + "boundary=".len();
    let v = content_type[at..].trim();
    let v = v.trim_matches('"');
    let v = v.split(';').next().unwrap_or("").trim();
    if v.is_empty() {
        None
    } else {
        Some(v.trim_matches('"').to_string())
    }
}

fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<Received>> {
    let delim = format!("\r\n--{boundary}").into_bytes();
    // Первая граница приходит без предшествующего CRLF — дописываем его сами,
    // чтобы по всему телу искать один и тот же разделитель.
    let mut buf = Vec::with_capacity(body.len() + 2);
    buf.extend_from_slice(b"\r\n");
    buf.extend_from_slice(body);

    let mut pos = find(&buf, &delim, 0).ok_or_else(|| anyhow!("границы частей не найдены"))?;
    let mut out = Vec::new();
    loop {
        let start = pos + delim.len();
        if buf[start..].starts_with(b"--") {
            break; // закрывающая граница
        }
        let next =
            find(&buf, &delim, start).ok_or_else(|| anyhow!("отправка оборвалась на середине"))?;
        let part = &buf[start..next];
        if let Some(h) = find(part, b"\r\n\r\n", 0) {
            if let Some(name) = filename(&part[..h]) {
                out.push(Received {
                    name,
                    bytes: part[h + 4..].to_vec(),
                });
            }
        }
        pos = next;
    }
    Ok(out)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < from + needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn filename(headers: &[u8]) -> Option<String> {
    let owned = String::from_utf8_lossy(headers);
    let h: &str = &owned;
    // Только ASCII-регистр: длина в байтах не меняется, значит смещение
    // из копии можно применить к оригиналу.
    let at = h.to_ascii_lowercase().find("filename=\"")? + "filename=\"".len();
    let rest = &h[at..];
    let raw = &rest[..rest.find('"')?];
    if raw.is_empty() {
        None
    } else {
        Some(safe_name(raw))
    }
}

/// Имя приходит с телефона и уйдёт в имя файла на диске — чистим путь и всё,
/// что Windows не переварит.
fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .take(120)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "снимок.jpg".to_string()
    } else {
        cleaned
    }
}

// ── что принимаем ───────────────────────────────────────────────────────────

fn is_media(name: &str, bytes: &[u8]) -> bool {
    !bytes.is_empty() && (signature_ok(bytes) || ext_ok(name))
}

fn signature_ok(b: &[u8]) -> bool {
    b.starts_with(b"\x89PNG\r\n\x1a\n")
        || b.starts_with(&[0xFF, 0xD8, 0xFF])
        || b.starts_with(b"GIF8")
        || b.starts_with(b"BM")
        || b.starts_with(b"II*\0")
        || b.starts_with(b"MM\0*")
        || b.starts_with(b"%PDF")
        || (b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP")
        // HEIC с айфона: тип файла лежит четвёртым байтом после размера бокса.
        || (b.len() >= 12
            && &b[4..8] == b"ftyp"
            && matches!(
                &b[8..12],
                b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif"
            ))
}

fn ext_ok(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "heic" | "heif" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "pdf"
    )
}

// ── QR ──────────────────────────────────────────────────────────────────────

fn qr_png_for(text: &str) -> Result<Vec<u8>> {
    let code =
        QrCode::new(text.as_bytes()).map_err(|e| anyhow!("не удалось создать QR-код: {e}"))?;
    let w = code.width();
    let modules: Vec<u8> = code
        .to_colors()
        .iter()
        .map(|c| c.select(0u8, 255u8))
        .collect();

    // Поле в 4 модуля обязательно по стандарту: без него камера не ловит код.
    const SCALE: u32 = 10;
    const QUIET: u32 = 4;
    let side = (w as u32 + QUIET * 2) * SCALE;

    let img = image::GrayImage::from_fn(side, side, |x, y| {
        let (mx, my) = (x / SCALE, y / SCALE);
        let inside = mx >= QUIET && my >= QUIET && mx < QUIET + w as u32 && my < QUIET + w as u32;
        let v = if inside {
            modules[(my - QUIET) as usize * w + (mx - QUIET) as usize]
        } else {
            255
        };
        image::Luma([v])
    });

    let mut png: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| anyhow!("не удалось нарисовать QR-код: {e}"))?;
    Ok(png.into_inner())
}

// ── страница для телефона ───────────────────────────────────────────────────

// Ни одной внешней ссылки: в клинике телефон бывает без интернета, а страницу
// отдаёт наш же сервер — грузиться ей больше неоткуда.
const CSS: &str = r#"
:root{color-scheme:light dark;--bg:#ffffff;--fg:#14181d;--muted:#5a636d;--brand:#0b5fb0;--on-brand:#ffffff;--line:#c9cdd3}
@media (prefers-color-scheme:dark){:root{--bg:#14171a;--fg:#f2f4f6;--muted:#b9c0c8;--brand:#7ab5f0;--on-brand:#0d1b2a;--line:#3a4149}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:18px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
main{max-width:32rem;margin:0 auto;padding:24px 16px 48px}
h1{font-size:40px;line-height:1.15;margin:8px 0 16px;font-weight:700}
p{margin:0 0 16px}
.muted{color:var(--muted);font-size:16px}
.pick,.send{display:flex;align-items:center;justify-content:center;text-align:center;width:100%;min-height:72px;padding:16px 20px;margin:0 0 16px;border-radius:14px;font:inherit;font-size:22px;font-weight:600;text-decoration:none;border:2px solid var(--brand)}
.pick{position:relative;background:transparent;color:var(--brand)}
.pick input{position:absolute;inset:0;width:100%;height:100%;opacity:0}
.send{background:var(--brand);color:var(--on-brand);cursor:pointer}
.pick:focus-within,.send:focus-visible{outline:3px solid var(--fg);outline-offset:3px}
.bad{border-left:4px solid var(--line);padding-left:12px}
ul{margin:0 0 16px;padding-left:22px}
li{margin-bottom:4px;overflow-wrap:anywhere}
"#;

fn shell(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{}</title><style>{}</style></head><body><main>{}</main></body></html>",
        title, CSS, body
    )
}

fn page_form() -> String {
    shell(
        "Снимки в программу",
        r#"<h1>Снимки в программу</h1>
<p>Выберите снятые страницы — они уйдут прямо в открытую программу на компьютере.</p>
<form method="post" enctype="multipart/form-data">
<label class="pick"><span>Выбрать снимки</span><input type="file" name="file" accept="image/*" multiple></label>
<button class="send" type="submit">Отправить</button>
</form>
<p class="muted">Можно выбрать сразу несколько. За один раз — до 40 МБ.</p>
<script>
var i=document.querySelector('.pick input'),s=document.querySelector('.pick span');
i.addEventListener('change',function(){s.textContent=i.files.length?'Выбрано снимков: '+i.files.length:'Выбрать снимки';});
</script>"#,
    )
}

fn page_done(ok: usize, bad: &[String]) -> String {
    let mut b = String::new();
    if ok > 0 {
        b.push_str("<h1>Готово, снимки ушли в программу</h1>");
        b.push_str(&format!("<p>Принято: {ok}.</p>"));
    } else if bad.is_empty() {
        b.push_str(
            "<h1>Снимки не выбраны</h1>\
<p>Нажмите «Выбрать снимки», отметьте нужные страницы и отправьте ещё раз.</p>",
        );
    } else {
        b.push_str("<h1>Ничего не принято</h1>");
    }
    if !bad.is_empty() {
        b.push_str(
            "<p class=\"bad\">Программа берёт только фотографии и PDF, поэтому не приняты:</p><ul>",
        );
        for n in bad {
            b.push_str(&format!("<li>{}</li>", esc(n)));
        }
        b.push_str("</ul>");
    }
    b.push_str("<a class=\"send\" href=\"./\">Отправить ещё</a>");
    shell("Готово", &b)
}

fn page_error(msg: &str) -> String {
    shell(
        "Не получилось",
        &format!(
            "<h1>Не получилось</h1><p>{}</p><a class=\"send\" href=\"./\">Попробовать ещё раз</a>",
            esc(msg)
        ),
    )
}

/// Имя файла приходит с чужого устройства и попадает в разметку — экранируем.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── проверки ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "----WebKitFormBoundaryTest";

    /// Собираем тело ровно так, как его шлёт Safari с айфона.
    fn body(boundary: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut v = Vec::new();
        for (name, data) in files {
            v.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            v.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
                     Content-Type: image/jpeg\r\n\r\n"
                )
                .as_bytes(),
            );
            v.extend_from_slice(data);
            v.extend_from_slice(b"\r\n");
        }
        v.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        v
    }

    #[test]
    fn one_photo_keeps_its_name_and_bytes() {
        // В теле снимка попадается и CRLF, и обрывок «--» — разбор не должен на них реагировать.
        let data: &[u8] = b"\xff\xd8\xff\xe0 tel snimka \r\n --nope-- hvost";
        let got = parse_multipart(&body(B, &[("IMG_0001.JPG", data)]), B).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "IMG_0001.JPG");
        assert_eq!(got[0].bytes.as_slice(), data);
    }

    #[test]
    fn two_photos_give_two_entries() {
        let a: &[u8] = b"\xff\xd8\xffAAA";
        let b: &[u8] = b"\x89PNG\r\n\x1a\nBB";
        let got = parse_multipart(&body(B, &[("a.jpg", a), ("b.png", b)]), B).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "a.jpg");
        assert_eq!(got[0].bytes.as_slice(), a);
        assert_eq!(got[1].name, "b.png");
        assert_eq!(got[1].bytes.as_slice(), b);
    }

    #[test]
    fn path_is_stripped_from_the_name() {
        let d: &[u8] = b"\xff\xd8\xffZ";
        let got = parse_multipart(&body(B, &[("C:\\Users\\x\\снимок.jpg", d)]), B).unwrap();
        assert_eq!(got[0].name, "снимок.jpg");
    }

    #[test]
    fn broken_boundary_is_an_error_not_a_panic() {
        let d: &[u8] = b"\xff\xd8\xffA";
        let b = body(B, &[("a.jpg", d)]);
        assert!(parse_multipart(&b, "другая-граница").is_err());
        // оборванная на середине отправка — тоже ошибка, а не падение
        assert!(parse_multipart(&b[..b.len() / 2], B).is_err());
        assert!(parse_multipart(b"", B).is_err());
        assert!(parse_multipart(b"\r\n--x\r\nhvost bez granicy", "x").is_err());
    }

    #[test]
    fn boundary_is_read_from_the_header() {
        assert_eq!(
            boundary_of("multipart/form-data; boundary=----WebKitFormBoundaryAbC").as_deref(),
            Some("----WebKitFormBoundaryAbC")
        );
        assert_eq!(
            boundary_of("multipart/form-data; boundary=\"xy\"").as_deref(),
            Some("xy")
        );
        assert!(boundary_of("application/json").is_none());
    }

    #[test]
    fn oversized_upload_is_refused() {
        let big = [7u8; 100];
        assert!(read_body(&big[..], 50).unwrap().is_none());
        assert_eq!(read_body(&big[..], 100).unwrap().unwrap().len(), 100);
        assert_eq!(read_body(&big[..], 1000).unwrap().unwrap().len(), 100);
    }

    #[test]
    fn only_photos_and_pdf_are_accepted() {
        assert!(is_media("IMG_1.JPG", b"\xff\xd8\xff\x00"));
        assert!(is_media("scan.pdf", b"%PDF-1.7"));
        assert!(is_media("IMG_2.heic", b"\x00\x00\x00\x18ftypheic"));
        assert!(!is_media("virus.exe", b"MZ\x90\x00"));
        assert!(!is_media("empty.jpg", b"")); // пустой файл сохранять незачем
    }

    #[test]
    fn qr_is_a_valid_png() {
        let png = qr_png_for("http://192.168.1.5:8734/").unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.len() > 100);
    }

    /// Ровно то, что увидел бы Safari на айфоне. Ловит белую страницу: пустое
    /// тело, потерянный Content-Type, забытый charset, битый UTF-8, форму,
    /// которой нужен JavaScript, — и заодно то, что напечатанный в QR адрес
    /// действительно обслуживается.
    #[test]
    fn get_gives_a_real_page_with_content_type_and_charset() {
        use std::io::Write;

        let Ok(intake) = Intake::start() else {
            return; // на машине сборки сети может не быть — проверять нечего
        };
        let url = intake.url().to_string();
        let (host, path) = url
            .trim_start_matches("http://")
            .split_once('/')
            .expect("адрес из QR без пути");
        let addr: SocketAddr = host
            .parse()
            .unwrap_or_else(|e| panic!("адрес из QR неразбираем: {url} ({e})"));

        let mut s = std::net::TcpStream::connect(addr)
            .unwrap_or_else(|e| panic!("по адресу из QR никто не слушает: {addr} ({e})"));
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        write!(
            s,
            "GET /{path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();

        // tiny_http умеет держать соединение открытым и после ответа, поэтому
        // читаем до конца разметки; упёрлись в таймаут — значит ответ оборван,
        // и это покажут проверки ниже.
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = s.read(&mut chunk) {
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
            if raw.ends_with(b"</html>") {
                break;
            }
        }

        let text = String::from_utf8(raw).expect("страница должна быть корректным UTF-8");
        let (head, body) = text
            .split_once("\r\n\r\n")
            .expect("после заголовков нет пустой строки");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let low = head.to_ascii_lowercase();
        assert!(low.contains("content-type: text/html"), "{head}");
        assert!(low.contains("charset=utf-8"), "{head}");

        assert!(body.len() > 500, "тело страницы почти пустое: {body:?}");
        assert!(body.contains("Снимки в программу"), "{body}");
        assert!(
            body.trim_end().ends_with("</html>"),
            "разметка оборвана: {body}"
        );
        // Форма должна работать без JavaScript: выбор файла и отправка — сами по себе.
        assert!(body.contains("enctype=\"multipart/form-data\""), "{body}");
        assert!(body.contains("type=\"file\""), "{body}");
        assert!(body.contains("type=\"submit\""), "{body}");

        let mut unknown = std::net::TcpStream::connect(addr).unwrap();
        unknown
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        write!(
            unknown,
            "GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = [0u8; 128];
        let n = unknown.read(&mut response).unwrap();
        assert!(String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 404"));
    }

    /// Главное, ради чего кнопка «Проверить приём» вообще появилась: она
    /// обязана отличать работающий сервер от заглушённого. Иначе она врёт, и
    /// врач по её «всё в порядке» ищет поломку не там.
    #[test]
    fn selfcheck_tells_a_live_server_from_a_dead_one() {
        let Ok(mut intake) = Intake::start() else {
            return; // на машине сборки сети может не быть — проверять нечего
        };
        let port = intake.port;

        // Петля обязана отвечать всегда: сервер слушает 0.0.0.0, и если уж он
        // поднялся, сам с собой соединиться он может.
        if let Err(e) = probe(
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port),
            &intake.token,
        ) {
            panic!("сервер не отвечает сам себе на 127.0.0.1:{port} — {e}");
        }

        // А вот требовать ответа от КАЖДОГО адреса нельзя, и это не поблажка
        // тесту. Проверено на живой машине: адрес VPN-адаптера (10.0.85.2)
        // висит в списке, но соединение к нему с той же машины отваливается
        // по таймауту — у таких адаптеров своя маршрутизация. То есть
        // «слушаем на всех адресах» и «до каждого адреса можно достучаться» —
        // разные вещи. Ровно поэтому врачу и дана кнопка «Проверить приём»:
        // она проверяет ТОТ адрес, который сейчас в QR-коде, а не веру в то,
        // что раз сервер поднялся, то доступен отовсюду.
        let live = interfaces()
            .into_iter()
            .filter(|(_, ip)| probe(SocketAddr::new(*ip, port), &intake.token).is_ok())
            .count();
        if !interfaces().is_empty() {
            assert!(live > 0, "ни один сетевой адрес машины не ответил");
        }

        // В списке для врача петли быть не должно: с телефона до неё не дойти.
        assert!(intake.addresses().iter().all(|(_, ip)| !ip.is_loopback()));
        assert!(intake.addresses().iter().all(|(name, _)| name != "lo"));

        intake.check();
        assert!(
            matches!(intake.last_check(), Some((true, _))),
            "живой сервер проверка не увидела: {:?}",
            intake.last_check()
        );

        let addr = SocketAddr::new(intake.ip(), port);
        let token = intake.token.clone();
        drop(intake); // глушим сервер
        let dead = probe(addr, &token);
        assert!(dead.is_err(), "заглушённый сервер проверка сочла живым");
    }

    /// Ради чего всё затевалось: живой сервер и закрытый брандмауэр — это два
    /// РАЗНЫХ ответа, а раньше был один. Сети тест не трогает.
    #[test]
    fn a_live_server_nobody_can_reach_is_not_the_same_as_a_working_one() {
        let ip = IpAddr::from([10, 20, 4, 107]);
        let url = "http://10.20.4.107:40445/";

        let silent = diagnose(Reach::Serving, ip, 40445, url, 0, 0);
        let working = diagnose(Reach::Serving, ip, 40445, url, 3, 2);
        assert_ne!(silent.text, working.text, "оба исхода дали один текст");

        // Сервер отвечает — значит, ok, иначе врач пойдёт перезапускать
        // программу, которая исправна. Но текст обязан сказать, что не так.
        assert!(silent.ok);
        assert!(silent.text.contains("ни одного обращения"));
        assert!(silent.text.contains("Режим модема"), "нет запасного пути");
        assert!(silent.text.contains("мышкой"), "нет перетаскивания");
        assert!(
            silent.text.contains("10.20"),
            "врачу не с чем сличить сеть телефона"
        );
        // Команда — только в этом исходе, и обязательно с пояснением.
        let cmd = silent.command.expect("нет команды для админа");
        assert!(
            cmd.contains("40445"),
            "команда открывает не тот порт: {cmd}"
        );
        assert!(silent.command_note.is_some());
        assert!(
            working.command.is_none(),
            "исправной машине команда ни к чему"
        );
    }

    /// Четыре беды — четыре разных ответа, и ни один не предлагает лечить не то.
    #[test]
    fn every_failure_gets_its_own_words() {
        let ip = IpAddr::from([10, 20, 4, 107]);
        let url = "http://10.20.4.107:40445/";
        let all = [
            diagnose(Reach::NoNetwork, ip, 40445, url, 0, 0),
            diagnose(Reach::ServerDown("порт занят".into()), ip, 40445, url, 0, 0),
            diagnose(
                Reach::AddressUnreachable("таймаут".into()),
                ip,
                40445,
                url,
                0,
                0,
            ),
            diagnose(Reach::Serving, ip, 40445, url, 0, 0),
            diagnose(Reach::Serving, ip, 40445, url, 2, 0),
            diagnose(Reach::Serving, ip, 40445, url, 2, 2),
        ];
        let mut texts: Vec<&str> = all.iter().map(|c| c.text.as_str()).collect();
        texts.sort_unstable();
        let n = texts.len();
        texts.dedup();
        assert_eq!(texts.len(), n, "два исхода звучат одинаково");

        // Первые три — поломки, и врач должна это видеть.
        assert!(all[..3].iter().all(|c| !c.ok));
        // Последние три — сервер отвечает, перезапускать нечего.
        assert!(all[3..].iter().all(|c| c.ok));
        // Ни один текст не отправляет врача к брандмауэру без нужды.
        assert!(all[1].command.is_none() && all[2].command.is_none());

        // Ошибку связи показываем, иначе админу не за что зацепиться.
        assert!(all[1].text.contains("порт занят"));
        assert!(all[2].text.contains("таймаут"));
        // Обращения есть, снимков нет — сеть тут ни при чём, и так и сказано.
        assert!(all[4].text.contains("ни при чём"));
    }

    /// Команда уходит человеку в руки: перепутанный порт или инструмент —
    /// это поход к админу впустую.
    #[test]
    fn the_command_names_the_real_port_and_the_real_tool() {
        let (cmd, note) = open_port_command(40445);
        assert!(cmd.contains("40445") && note.contains("40445"));
        assert!(
            !note.is_empty() && note.lines().count() == 1,
            "пояснение не в одну строку"
        );
        if cfg!(windows) {
            assert!(cmd.starts_with("netsh"));
        } else if std::path::Path::new("/usr/bin/firewall-cmd").exists() {
            assert!(cmd.contains("firewall-cmd --permanent --add-port=40445/tcp"));
        } else {
            assert!(cmd.contains("iptables -I INPUT -p tcp --dport 40445"));
        }
    }

    /// Самопроверка не должна накручивать счётчик обращений: иначе кнопка
    /// «Проверить приём» сама себе доказывает, что телефон доходит.
    #[test]
    fn the_self_check_does_not_count_as_a_phone() {
        let Ok(mut intake) = Intake::start() else {
            return;
        };
        assert_eq!(intake.hits(), 0);
        intake.check();
        intake.check();
        assert_eq!(intake.hits(), 0, "проверка засчитала саму себя за телефон");

        // А обычный заход — засчитывает.
        let addr = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), intake.port);
        if let Ok(mut s) = std::net::TcpStream::connect(addr) {
            use std::io::Write;
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let _ = write!(
                s,
                "GET /{}/ HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n",
                intake.token
            );
            // Счётчик растёт до ответа, но дождаться ответа надёжнее, чем спать.
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = s.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                if raw.ends_with(b"</html>") {
                    break;
                }
            }
            assert_eq!(intake.hits(), 1, "настоящее обращение не засчитано");
        }
    }

    /// Журнал — единственное, что врач может прислать: терминала у неё нет.
    #[test]
    fn the_journal_says_where_the_server_is() {
        // Журнал обнуляется один раз за запуск программы, и в тестах эта
        // единственная урезка может прийтись ровно на нашу запись: соседние
        // проверки поднимают свои серверы в соседних потоках. Повторяем —
        // второй урезки за запуск уже не случится.
        let mut missing = String::new();
        for _ in 0..3 {
            let Ok(intake) = Intake::start() else {
                return; // на машине сборки сети может не быть
            };
            let addr = format!("{}:{}", intake.ip(), intake.port);
            drop(intake);

            let raw = std::fs::read(crate::settings::dir().join("журнал.txt")).unwrap_or_default();
            let log = String::from_utf8_lossy(&raw);
            let want = [
                "приём: сервер слушает 0.0.0.0:".to_string(),
                "приём: сети машины:".to_string(),
                format!("приём: QR-код для {addr} готов"),
                "приём: выключен, снимков принято всего 0".to_string(),
            ];
            missing = want
                .iter()
                .filter(|e| !log.contains(e.as_str()))
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            if missing.is_empty() {
                return;
            }
        }
        panic!("в журнале нет строк: {missing}");
    }

    #[test]
    fn start_survives_a_machine_without_network() {
        // На машине сборки сети может не быть — это не повод падать.
        match Intake::start() {
            Ok(i) => {
                assert!(i.url().starts_with("http://") && i.url().ends_with('/'));
                assert_eq!(i.count(), 0);
                assert!(i.take().is_empty());
                assert!(i.qr_png().unwrap().starts_with(b"\x89PNG"));
            }
            Err(e) => assert!(!e.to_string().is_empty()),
        }
    }
}
