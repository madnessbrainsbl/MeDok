//! Окно программы.
//!
//! Порядок блоков повторяет порядок работы: перетащили направление ->
//! проверили пациента -> дописали осмотр -> сохранили.

use crate::ink::{self, Tint};
use crate::odt::Napravlenie;
use crate::settings::Settings;
use crate::ui;
use crate::vypiska::Vypiska;
use crate::{fmt, norms, odt};
use crate::{intake, package, store};
use anyhow::Context;
use eframe::egui;

/// Всё, что врач вводит руками на конкретного ребёнка.
///
/// Отдельной структурой намеренно: при загрузке нового направления она
/// заменяется целиком, чтобы данные предыдущего пациента не попадали
/// в следующую выписку.
#[derive(Default, Clone, PartialEq)]
struct Patient {
    /// Где ребёнок живёт на самом деле.
    ///
    /// Подставляется из направления, но правится руками: в КУМке бывает
    /// прописка вместо фактического адреса, а исправить её врач до этого
    /// не могла никак — что пришло, то и уходило в другое учреждение.
    address: String,
    diagnosis: String,
    phone: String,
    snils: String,
    birth_cert: String,
    organized: String,
    complaints: String,
    chdd: String,
    chss: String,
    anamnesis_disease: String,
    anamnesis_life: String,
    referral_note: String,
    attachments: String,
}

impl Patient {
    /// Заготовка анамнеза жизни подставляется сразу, чтобы врач проверил
    /// и заполнил её при приёме.
    fn new(life_template: &str) -> Self {
        Self {
            attachments: crate::vypiska::ATTACHMENTS_DEFAULT.into(),
            anamnesis_life: life_template.to_string(),
            ..Default::default()
        }
    }
}

pub struct App {
    set: Settings,
    nap: Option<Napravlenie>,
    src_name: String,
    pat: Patient,

    /// Чернила режутся один раз при смене файла, а не на каждое сохранение:
    /// распаковка 40-мегапиксельного снимка занимает больше секунды.
    stamp_png: Option<Vec<u8>>,
    sign_png: Option<Vec<u8>>,
    ink_errs: Vec<String>,

    status: Status,
    show_settings: bool,
    /// Окно «О программе»: авторство и условия использования.
    show_about: bool,
    /// Копия настроек на момент открытия окна — для «Отмены».
    set_backup: Option<Settings>,
    /// Врача удаляем в два нажатия: его печатью подписывают документы,
    /// и промах мимо соседней кнопки не должен стирать профиль.
    confirm_delete: bool,
    /// Высоту окна подгоняем под экран один раз на первом кадре.
    sized: bool,
    /// Чернила вырезаются после первого кадра, а не при запуске.
    ink_ready: bool,
    /// Снимок полей на прошлом кадре: по нему гасим устаревший статус.
    seen: Patient,

    tab: Tab,
    /// Приём снимков с телефона. Поднимается по требованию, а не при запуске:
    /// открытый порт нужен только пока врач фотографирует.
    intake: Option<intake::Intake>,
    qr: Option<egui::TextureHandle>,
    /// Из чего собирается пакет, в порядке страниц.
    sources: Vec<package::Source>,
    quality: package::Quality,
    /// Карточка этого ребёнка из прошлого визита — предлагаем подставить.
    found_card: Option<store::Card>,
    /// Карточка есть, но не открылась: чужая учётная запись Windows или
    /// битый файл. Пустое место вместо неё читается как «ребёнок новый»,
    /// и врач затрёт накопленный за годы анамнез — поэтому фраза, а не
    /// молчание.
    card_err: Option<String>,
    /// Файлы, которые добавятся в пакет следующими кадрами.
    ///
    /// Направление переводится в PDF внешним LibreOffice, и первый его запуск
    /// занимает секунды. Сделать это прямо в обработке перетаскивания значит
    /// заморозить окно раньше, чем врач увидит хоть слово о том, что идёт
    /// работа: нарисовать что-либо посреди кадра в egui нельзя. Поэтому здесь
    /// только откладываем, а рисуем «Перевожу…» — и берёмся за файл кадром
    /// позже, когда надпись уже на экране.
    pending_source: Vec<(std::path::PathBuf, package::Kind)>,
}

#[derive(Debug)]
enum Status {
    Idle,
    Ok(String),
    Err(String),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Vypiska,
    Package,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        set_cyrillic_font(&cc.egui_ctx);
        ui::apply_style(&cc.egui_ctx);
        let set = Settings::load();
        let life = set.life_template.clone();
        let app = Self {
            set,
            nap: None,
            src_name: String::new(),
            pat: Patient::new(&life),
            stamp_png: None,
            sign_png: None,
            ink_errs: Vec::new(),
            status: Status::Idle,
            show_settings: false,
            show_about: false,
            set_backup: None,
            confirm_delete: false,
            sized: false,
            ink_ready: false,
            seen: Patient::new(&life),
            tab: Tab::Vypiska,
            intake: None,
            qr: None,
            sources: Vec::new(),
            quality: package::Quality::Normal,
            found_card: None,
            card_err: None,
            pending_source: Vec::new(),
        };
        // Чернила режем не здесь: распаковка присланной подписи (6280x6484)
        // занимает больше секунды, и окно всё это время не нарисовано —
        // выглядит как зависший чёрный прямоугольник при запуске.
        app
    }

    /// Пересчитать печать и подпись из файлов активного врача.
    fn recut_ink(&mut self) {
        self.ink_errs.clear();
        // Пути снимаем до замыкания — оно держит `ink_errs`. Заодно это снимок
        // именно активного профиля: после смены врача чужие чернила не живут.
        let stamp = self.set.doc().stamp_path.clone();
        let sign = self.set.doc().sign_path.clone();
        let mut cut = |path: &str, tint: Tint, what: &str| -> Option<Vec<u8>> {
            if path.trim().is_empty() {
                return None;
            }
            match ink::load(std::path::Path::new(path))
                .and_then(|img| ink::extract(&img, ink::THRESHOLD_DEFAULT, tint))
                .and_then(|i| ink::to_png(&i))
            {
                Ok(png) => Some(png),
                Err(e) => {
                    self.ink_errs.push(format!("{what}: {e:#}"));
                    None
                }
            }
        };
        self.stamp_png = cut(&stamp, Tint::Blue, "печать");
        self.sign_png = cut(&sign, Tint::Keep, "подпись");
    }

    /// Открыть направление: заполнить поля и положить его в пакет. Так его
    /// открывают кнопка «Выбрать файл…» и «Открыть с помощью» из проводника.
    pub fn open_path(&mut self, path: &std::path::Path) {
        if self.open_odt(path) {
            self.queue_source(path, package::Kind::Referral);
        }
    }

    /// Загрузить направление, НЕ стирая того, что врач уже вписала руками.
    ///
    /// Раньше любое направление обнуляло поля осмотра: логика была «новое
    /// направление — новый ребёнок, чужие ЧДД и ЧСС в его выписку попасть не
    /// должны». Довод верный, но правило оказалось шире причины. Врач
    /// работает не в том порядке, в каком я предполагал: она вписывает
    /// осмотр, анамнез, цифры — и только потом вспоминает про направление.
    /// Её слова: «если собираешь не в той последовательности, то инфа из
    /// окошек скипается — загружаю направление после заполнения анамнеза,
    /// ЧСС, ЧД — всё слетает». Приём переделывать себя под программу она не
    /// обязана.
    ///
    /// Поэтому чистим ровно тогда, когда есть что перепутать: направление
    /// уже было загружено И новое — на другого ребёнка. Если до этого
    /// направления не было, стирать нечего: всё, что в полях, врач вписала
    /// сама, глядя на этого же ребёнка.
    fn open_odt(&mut self, path: &std::path::Path) -> bool {
        match odt::parse_file(path) {
            Ok(n) => {
                let previous = self.nap.as_ref().map(|old| old.fio.clone());
                let another_child = previous
                    .as_deref()
                    .map(|old| !same_person(old, &n.fio))
                    .unwrap_or(false);
                if another_child {
                    // Вот здесь стирать обязательно: цифры осмотра в выписке
                    // за её подписью относились бы к другому ребёнку.
                    self.pat = Patient::new(&self.set.life_template);
                    self.found_card = None;
                    // И пакет — по той же причине, и это даже опаснее полей:
                    // в нём лежат направление, выписка и обследования
                    // прошлого ребёнка. Пакет не чистился между детьми вовсе,
                    // и следующий собранный ушёл бы в краевую с чужими
                    // документами. Поля врач хотя бы видит глазами, а
                    // страницы пакета — только на второй вкладке.
                    self.sources.clear();
                    self.pending_source.clear();
                }
                // Адрес из направления — подсказка, а не истина, и вписанное
                // руками он не перебивает: в КУМке бывает прописка вместо
                // места, где ребёнок живёт на самом деле.
                if self.pat.address.trim().is_empty() {
                    self.pat.address = n.address_by_fact().to_string();
                }
                self.src_name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // Говорим вслух, что стало с уже вписанным: молчание здесь
                // читается как «программа съела мой осмотр».
                self.status = Status::Ok(match previous {
                    Some(old) if another_child => format!(
                        "Загружен другой ребёнок: {}. Поля осмотра очищены и пакет тоже — всё прежнее относилось к {old}",
                        n.fio
                    ),
                    _ => format!("Загружено: {}. Вписанное вами сохранено", n.fio),
                });
                self.nap = Some(n);
                // Фамилию врача из направления не подставляем: там стоит тот,
                // кто выписал направление, а подписывает выписку другой человек.
                true
            }
            Err(e) => {
                // Не оставляем прошлого пациента загруженным — иначе врач
                // выпустит полную выписку на предыдущего ребёнка.
                self.nap = None;
                self.src_name.clear();
                self.status = Status::Err(format!("{e:#}"));
                false
            }
        }
    }

    /// Положить страницы выписки в пакет — на её место, за направлением.
    ///
    /// Отдельным методом, а не куском `save`: сохранение поднимает окно
    /// выбора файла, и проверить порядок страниц через него нельзя. А порядок
    /// здесь ломался уже однажды — выписка ложилась в конец, за обследования.
    fn put_vypiska_in_package(&mut self) {
        // Выписка кладётся в пакет страницами-картинками, а не своим PDF:
        // она векторная, и вытащить из неё «снимок страницы» нельзя —
        // самой крупной вложенной картинкой там оказывается печать.
        self.sources.retain(|s| s.kind != package::Kind::Vypiska);
        match crate::vypiska::to_images(self.build_signed(), 1600) {
            Ok(imgs) => {
                for (i, img) in imgs.iter().enumerate() {
                    let mut buf = std::io::Cursor::new(Vec::new());
                    if img.write_to(&mut buf, image::ImageFormat::Png).is_ok() {
                        self.sources.push(package::Source {
                            name: format!("выписка, стр. {}", i + 1),
                            bytes: buf.into_inner(),
                            kind: package::Kind::Vypiska,
                        });
                    }
                }
            }
            Err(e) => self.status = Status::Err(format!("выписка не легла в пакет: {e:#}")),
        }
        package::in_order(&mut self.sources);
    }

    /// Напечатать выписку, не заставляя врача сперва её сохранять и искать.
    ///
    /// Файл всё равно кладём на диск: напечатанный документ должен остаться и
    /// на диске тоже, а принтеру нужен путь, а не поток. Кладём без вопросов,
    /// в папку программы: окно «куда сохранить» здесь было бы третьим лишним
    /// шагом между «нажала» и «поехало на бумагу».
    fn print_vypiska(&mut self) {
        let v = self.build_signed();
        let name = v
            .fio
            .split_whitespace()
            .next()
            .unwrap_or("выписка")
            .to_string();
        let dir = crate::settings::dir().join("документы");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status = Status::Err(format!("не создать папку {}: {e}", dir.display()));
            return;
        }
        let path = dir.join(format!("{name} выписка.pdf"));
        let made = crate::vypiska::to_pdf(v).and_then(|pdf| {
            std::fs::write(&path, &pdf)?;
            Ok(())
        });
        if let Err(e) = made {
            self.status = Status::Err(format!("{e:#}"));
            return;
        }
        // Настройки, карточку и пакет — так же, как при сохранении: с точки
        // зрения приёма печать — это тоже «документ выпущен», и искать потом,
        // почему выписки нет в пакете, врач не должна.
        self.keep_settings();
        self.remember_card();
        self.put_vypiska_in_package();
        match print_file(&path) {
            Ok(()) => {
                self.status = Status::Ok(format!(
                    "Отправлено на принтер. Копия осталась здесь: {}",
                    path.display()
                ))
            }
            Err(e) => {
                // Документ уже на диске — говорим об этом в той же строке,
                // иначе отказ печати читается как «ничего не получилось».
                self.status = Status::Err(format!("{e:#}"));
                let _ = open_in_viewer(&path);
            }
        }
    }

    /// Документ без картинок печати — для проверки готовности каждый кадр.
    fn build(&self) -> Vypiska {
        let on = fmt::today();
        let p = &self.pat;
        let mut v = Vypiska::new();
        v.clinic = self.set.clinic.clone();
        if let Some(n) = &self.nap {
            v.fio = n.fio.clone();
            v.dob = fmt::dob_full(&n.dob, on);
            v.policy = n.policy.clone();
        }
        // Адрес — из поля, которое врач видит и правит: направление приносит
        // подсказку, а не истину. См. `Napravlenie::address_by_fact`.
        v.address = p.address.clone();
        v.doctor = self.set.doc().name.clone();
        // Заведующий — из направления: он разный на каждого пациента, и
        // разобран из настоящего документа. Профиль врача — только запасной
        // вариант, на случай если в направлении эта строка не разобралась.
        let from_referral = self
            .nap
            .as_ref()
            .map(|n| n.head_of_dept.clone())
            .unwrap_or_default();
        v.head_of_dept = if !from_referral.trim().is_empty() {
            from_referral
        } else {
            self.set.doc().head_of_dept.clone()
        };
        v.date = format!("{}г", on.format("%d.%m.%Y"));
        v.diagnosis = p.diagnosis.clone();
        v.phone = p.phone.clone();
        v.snils = p.snils.clone();
        v.birth_cert = p.birth_cert.clone();
        v.organized = p.organized.clone();
        v.complaints = p.complaints.clone();
        v.chdd = p.chdd.clone();
        v.chss = p.chss.clone();
        v.anamnesis_disease = p.anamnesis_disease.clone();
        v.anamnesis_life = p.anamnesis_life.clone();
        v.referral_note = p.referral_note.clone();
        v.attachments = p.attachments.clone();
        v.exam_template = self.set.exam_template.clone();
        v.stamp_cm = self.set.stamp_cm;
        v
    }

    /// Печать и подпись указаны, но не читаются — это отказ, а не мелочь:
    /// иначе документ уходит неподписанным, а строка статуса зелёная.
    fn ink_problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.set.overlay {
            return v;
        }
        let d = self.set.doc();
        if !d.stamp_path.trim().is_empty() && self.stamp_png.is_none() {
            v.push("печать не прочиталась".into());
        }
        if !d.sign_path.trim().is_empty() && self.sign_png.is_none() {
            v.push("подпись не прочиталась".into());
        }
        v
    }

    /// Печать или подпись не загружены вовсе — документ выйдет без них.
    ///
    /// Это не отказ: провести выписку бумагой законно, и мешать не надо.
    /// Но в готовом PDF на месте печати просто пусто, и узнать об этом врач
    /// может только по уже сохранённому файлу — «с печатью не поняла, её
    /// нет». Поэтому говорим до сохранения и словами, а не молчим.
    fn ink_not_loaded(&self) -> Option<String> {
        if !self.set.overlay {
            return None;
        }
        let d = self.set.doc();
        Some(
            match (
                d.stamp_path.trim().is_empty(),
                d.sign_path.trim().is_empty(),
            ) {
                (true, true) => "Печать и подпись не загружены — документ выйдет без них",
                (true, false) => "Печать не загружена — документ выйдет без неё",
                (false, true) => "Подпись не загружена — документ выйдет без неё",
                (false, false) => return None,
            }
            .to_string()
                + ". Настройки → Печать и подпись",
        )
    }

    fn problems(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .build()
            .missing()
            .iter()
            .map(|s| match *s {
                "врач" => "фамилия врача — в настройках".to_string(),
                other => other.to_string(),
            })
            .collect();
        // Стёртые подстановки — поломка не в полях, а в постоянном тексте
        // осмотра, и врач об этом не догадается: поля-то заполнены. Поэтому
        // одной общей строкой говорим, куда идти, и что чинится это одним
        // нажатием. Одной, а не к каждому имени: стирают их обычно все три
        // разом.
        if v.iter()
            .any(|p| p.starts_with(crate::vypiska::PLACEHOLDER_LOST))
        {
            v.push("починить: Настройки → Текст осмотра → «Вернуть исходный текст»".into());
        }
        v.extend(self.ink_problems());
        v
    }

    /// Документ вместе с печатью и подписью — в таком виде он и печатается.
    fn build_signed(&self) -> Vypiska {
        let mut v = self.build();
        if self.set.overlay {
            v.stamp = self.stamp_png.clone();
            v.sign = self.sign_png.clone();
        }
        v
    }

    fn save(&mut self) {
        let v = self.build_signed();
        let name = v
            .fio
            .split_whitespace()
            .next()
            .unwrap_or("выписка")
            .to_string();
        let Some((path, auto)) = ask_where_to_save(&format!("{name} выписка.pdf")) else {
            return;
        };
        match crate::vypiska::to_pdf(v).and_then(|pdf| {
            std::fs::write(&path, &pdf)?;
            Ok(pdf.len())
        }) {
            Ok(n) => {
                self.status = Status::Ok(format!(
                    "Сохранено — {:.0} КБ: {}{}",
                    n as f64 / 1024.0,
                    path.display(),
                    if auto {
                        " (окно выбора папки система не показала — файл положен сюда)"
                    } else {
                        ""
                    }
                ));
                // Момент, когда точно стоит закрепить настройки и карточку.
                self.keep_settings();
                self.remember_card();
                // Выписка кладётся в пакет страницами-картинками, а не своим PDF:
                // она векторная, и вытащить из неё «снимок страницы» нельзя —
                // самой крупной вложенной картинкой там оказывается печать.
                self.put_vypiska_in_package();
                let _ = open_in_viewer(&path);
            }
            Err(e) => self.status = Status::Err(format!("{e:#}")),
        }
    }
}

/// Один и тот же ли это ребёнок.
///
/// Сравниваем по словам, а не строку со строкой: в КУМке одно и то же ФИО
/// приходит то с двойным пробелом, то с разным регистром, и «Иванов  Иван»
/// не должно считаться другим ребёнком, чем «Иванов Иван». А вот сокращение
/// до инициалов — это уже другая запись, и её мы честно считаем другой:
/// лучше лишний раз очистить поля и сказать об этом, чем оставить чужие
/// цифры осмотра.
fn same_person(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        s.split_whitespace()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
    };
    words(a) == words(b)
}

/// Открыть готовый файл в системном приложении по умолчанию — врач должен
/// сразу увидеть, что получилось, а не искать файл руками в проводнике.
#[cfg(windows)]
fn open_in_viewer(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// `xdg-open` — тот же принцип на Linux: спрашивает у рабочего стола, чем
/// открывать PDF, и не зависит от того, GNOME это или KDE.
#[cfg(unix)]
fn open_in_viewer(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Отправить готовый PDF на принтер, минуя просмотрщик.
///
/// Зачем отдельная кнопка, если файл и так открывается в просмотрщике: врач
/// сказала прямо — «твоя программа не даёт печатать мне ничего». Путь через
/// просмотрщик держится на том, что в системе он есть, умеет печатать и врач
/// знает про Ctrl+P. Любое из трёх может не совпасть, и тогда документ есть,
/// а на бумаге его нет.
///
/// На Linux печатью заведует CUPS, и `lp` — его штатная команда. Мы НЕ
/// показываем своё окно выбора принтера: печатает тот, что назначен в системе
/// основным, — врач его уже выбрала один раз в настройках рабочего стола.
#[cfg(unix)]
fn print_file(path: &std::path::Path) -> anyhow::Result<()> {
    // `lp` и `lpr` — одна и та же печать через CUPS, просто разные команды
    // (System V и BSD). Обе лежат в пакете `cups-client`, но ставят его не
    // всегда: сам CUPS и печать из окон работают и без него — это проверено
    // на голом «Альте», где печать из программ есть, а `lp` нет. Поэтому
    // пробуем обе и только потом сдаёмся.
    let mut missing = 0;
    let mut last = None;
    for cmd in ["lp", "lpr"] {
        match std::process::Command::new(cmd).arg(path).output() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing += 1,
            other => {
                last = Some(other);
                break;
            }
        }
    }
    if missing == 2 {
        anyhow::bail!(
            "программе нечем отправить документ на принтер: в системе не установлен пакет cups-client. Сам принтер при этом работает — откройте сохранённый файл и напечатайте из просмотрщика (Ctrl+P), а чтобы кнопка заработала, попросите установить: apt-get install cups-client"
        )
    }
    match last.expect("после цикла ответ есть") {
        Err(e) => anyhow::bail!("не запустилась печать: {e}"),
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            // CUPS отвечает в stderr человеческим текстом, и самый частый —
            // «no default destination»: принтер в системе не выбран.
            let why = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if why.contains("no default destination") || why.contains("не задан") {
                anyhow::bail!(
                    "принтер в системе не выбран, поэтому печатать некуда. Меню → Параметры → Принтеры: выбрать свой и сделать его основным. Документ уже сохранён, он не потерян"
                )
            }
            anyhow::bail!("принтер не принял документ: {why}")
        }
    }
}

/// На Windows печать — это действие «Печать» у файла, то же, что в контекстном
/// меню проводника. Вызываем его через `Start-Process -Verb Print`, чтобы не
/// тянуть ради одной кнопки ручной вызов ShellExecute из WinAPI.
#[cfg(windows)]
fn print_file(path: &std::path::Path) -> anyhow::Result<()> {
    let arg = format!(
        "Start-Process -FilePath '{}' -Verb Print",
        path.display().to_string().replace("'", "''")
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &arg])
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
    anyhow::bail!(
        "напечатать не вышло: {why}. Документ сохранён — откройте его и напечатайте из просмотрщика (Ctrl+P)"
    )
}

/// Системный UI-шрифт с полной кириллицей: у egui в наборе по умолчанию она
/// неполная. На Windows — Segoe UI; на Linux того же нет, зато почти везде
/// стоит DejaVu Sans (в паре с fontconfig ставится как зависимость самого
/// иксового/Wayland-стека, а не только вручную). Если ни того ни этого нет —
/// остаёмся на встроенном, окно всё равно откроется.
fn set_cyrillic_font(ctx: &egui::Context) {
    let Some(bytes) = UI_FONT_FILES
        .iter()
        .filter_map(|n| crate::find_font(n))
        .find_map(|p| std::fs::read(p).ok())
    else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "ui".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "ui".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Что сказать, когда система не показала окно выбора файла. Отказ должен
/// называть и причину, и лекарство: врач сама пакет поставить не сможет —
/// машина в домене, — но передать эту строчку в техподдержку сможет.
const NO_DIALOGS: &str = "Система не открыла окно выбора файла: не установлен zenity. \
     Попросите поставить пакет zenity (одна команда: apt-get install zenity).";

/// Может ли система показать окно выбора файла.
///
/// На Linux `rfd` рисует диалог не сам: он идёт либо в `xdg-desktop-portal`
/// по D-Bus, либо запускает `zenity`. Если на машине нет ни того ни другого,
/// он просто возвращает `None` — неотличимо от «врач нажал Отмена».
///
/// Для программы, которая иначе не может сохранить документ ВООБЩЕ, это тихий
/// отказ в чистом виде: нажала «Сохранить», ничего не произошло, объяснения
/// нет. Замерено на живом Альте без `zenity`: `save_file()` возвращает `None`
/// за 196 микросекунд, молча. Поэтому наличие диалогов проверяем сами и, если
/// их нет, кладём документ в заранее известное место, а не теряем.
#[cfg(unix)]
fn dialogs_work() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        let in_path = |exe: &str| {
            std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
        };
        in_path("zenity")
            || in_path("kdialog")
            || std::path::Path::new(
                "/usr/share/dbus-1/services/org.freedesktop.portal.Desktop.service",
            )
            .exists()
    })
}

/// На Windows диалог рисует сама система, спрашивать нечего.
#[cfg(windows)]
fn dialogs_work() -> bool {
    true
}

/// Куда сохранить готовый документ. Возвращает путь и признак того, что
/// диалог показать не удалось и место выбрано за врача.
fn ask_where_to_save(file_name: &str) -> Option<(std::path::PathBuf, bool)> {
    if dialogs_work() {
        return rfd::FileDialog::new()
            .set_file_name(file_name)
            .add_filter("PDF", &["pdf"])
            .save_file()
            .map(|p| (p, false));
    }
    // Запасное место — папка «документы» рядом с самой программой: туда она
    // и так пишет свои данные, права там заведомо есть, и врач эту папку
    // видит. Куда именно лёг файл, скажет строка состояния.
    let dir = crate::settings::dir().join("документы");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("не создать папку для документов: {e}");
        return None;
    }
    Some((dir.join(file_name), true))
}

/// Записать в журнал всё, что понадобится для разбора отказа: какие шрифты
/// нашлись и есть ли чем показать диалоги.
///
/// Зовётся ДО открытия окна, а не из `App::new`: именно когда окно не
/// открылось, эти сведения и нужны, а до `App::new` дело тогда не доходит.
pub fn записать_среду() {
    crate::note(&format!(
        "шрифт бланка: {}",
        match crate::find_font(crate::vypiska::TIMES_FILES[0]) {
            Some(p) => p.display().to_string(),
            None => "системного нет, беру вшитый Libertinus".into(),
        }
    ));
    crate::note(&format!(
        "шрифт интерфейса: {}",
        match UI_FONT_FILES.iter().find_map(|n| crate::find_font(n)) {
            Some(p) => p.display().to_string(),
            None => "системного нет, беру набор egui".into(),
        }
    ));
    crate::note(&format!(
        "окна выбора файла: {}",
        if dialogs_work() {
            "есть"
        } else {
            "НЕТ (нужен zenity) — документы лягут в папку программы"
        }
    ));
}

/// Чем набирать интерфейс, в порядке предпочтения. Берётся первый найденный:
/// на Windows это Segoe UI, на Linux — DejaVu Sans (пакет `fonts-ttf-dejavu`
/// на «Альте», `fonts-dejavu-core` на Debian) либо Liberation Sans, если
/// DejaVu не поставлен. Ни одного нет — остаёмся на встроенном наборе egui.
#[cfg(windows)]
const UI_FONT_FILES: [&str; 1] = ["segoeui.ttf"];
#[cfg(unix)]
const UI_FONT_FILES: [&str; 2] = ["DejaVuSans.ttf", "LiberationSans-Regular.ttf"];

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Сначала — отложенное С ПРОШЛОГО кадра, и только потом новые файлы.
        // Порядок здесь и есть весь смысл отсрочки: брошенный файл встаёт в
        // очередь ниже, этот кадр рисует «Перевожу…», и лишь следующий
        // берётся за перевод. Когда очередь разбиралась после приёма новых
        // файлов, файл забирался в том же кадре — окно замирало на секунды,
        // так и не показав, что идёт работа.
        //
        // По одному файлу за кадр: бросают их пачкой, а перевод направления
        // занимает секунды — между файлами успевает обновиться счётчик.
        if !self.pending_source.is_empty() {
            let (p, kind) = self.pending_source.remove(0);
            self.add_source_from(&p, kind);
        }

        // Перетаскивание: файл можно бросить в любое место окна.
        let (hovering, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect::<Vec<_>>(),
            )
        });
        for p in &dropped {
            self.accept_dropped(p);
        }

        // Высота окна — от рабочей области экрана: жёсткие 500 pt прятали
        // ЧДД и ЧСС под сгиб на большом мониторе и не влезали на ноутбуке.
        if !self.sized {
            self.sized = true;
            if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
                let h = (m.y - 120.0).clamp(420.0, 720.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(900.0, h)));
                if let Some(c) = egui::ViewportCommand::center_on_screen(ctx) {
                    ctx.send_viewport_cmd(c);
                }
            }
        }

        // Зелёное «Сохранено» не должно висеть над уже изменённой формой:
        // на диске лежит прошлый файл, а экран утверждает, что всё сохранено.
        if self.pat != self.seen {
            self.seen = self.pat.clone();
            if matches!(self.status, Status::Ok(_)) {
                self.status = Status::Idle;
            }
        }

        // Первый кадр рисуем пустым, чернила режем сразу после него.
        if !self.ink_ready {
            self.ink_ready = true;
            ctx.request_repaint();
        } else if self.stamp_png.is_none()
            && self.sign_png.is_none()
            && self.ink_errs.is_empty()
            && !(self.set.doc().stamp_path.is_empty() && self.set.doc().sign_path.is_empty())
        {
            self.recut_ink();
        }

        self.drain_intake();
        if self.intake.is_some() {
            // Снимки приходят в другом потоке — окно должно обновляться само.
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }

        self.top_bar(ctx);
        let problems = self.problems();
        // Кнопка сохранения стоит в нижней панели и по Tab идёт раньше формы,
        // поэтому даём привычное сочетание.
        if problems.is_empty() && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
        {
            self.save();
        }
        self.bottom_bar(ctx, &problems);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(ui::PAD, 10)),
            )
            .show(ctx, |u| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(u, |u| {
                        // Зазор до полосы прокрутки: без него правый край поля
                        // сливается с ней.
                        u.set_width(u.available_width() - 8.0);
                        self.body(u, hovering)
                    });
            });

        self.settings_window(ctx);
        self.about_window(ctx);

        // Осталось что-то в очереди — нужен ещё кадр, и сам он не придёт:
        // egui перерисовывает окно по событиям, а врач после «Добавить
        // обследования…» просто ждёт, не двигая мышь.
        if !self.pending_source.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Текст осмотра правится в главном окне, а живёт в настройках.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.keep_settings();
    }
}

impl App {
    /// Закрепить настройки попутно — при сохранении, печати, закрытии окна.
    ///
    /// Отказ здесь не показываем окном: это не то действие, которое врач
    /// делала, и прерывать её ради него незачем. Но и не глотаем молча, как
    /// раньше: не сохранились настройки — значит, при следующем запуске не
    /// будет пути к печати или правленого текста осмотра, и без записи в
    /// журнале причину потом не найти.
    fn keep_settings(&self) {
        if let Err(e) = self.set.save() {
            crate::note(&format!("настройки не сохранились: {e:#}"));
        }
    }

    /// Кем подписывают. Стоит в шапке, а не только в настройках: смена врача
    /// меняет печать и подпись под документом, и видеть это надо всё время —
    /// подписать чужой фамилией нельзя.
    ///
    /// Обычный выпадающий список: до него достаёт Tab, открывается Enter или
    /// пробелом, высота кнопки — от `ui::apply_style` (26 pt, норма 24).
    /// `warn_unnamed` — писать ли под списком, что фамилия не заполнена.
    /// В шапке это нужно, а в самом окне настроек «фамилия не заполнена — в
    /// настройках» отсылает туда, где врач уже стоит, и только мешает
    /// прочитать строку про новый профиль.
    fn doctor_picker(&mut self, u: &mut egui::Ui, id: &str, warn_unnamed: bool) {
        // Имена копируем: список рисуется, когда `self.set` уже занят под
        // изменение активного индекса.
        let names: Vec<String> = self
            .set
            .doctors
            .iter()
            .map(|d| d.title().to_string())
            .collect();
        let current = self.set.doc().title().to_string();
        let unnamed = self.set.doc().name.trim().is_empty();
        let mut switched = false;
        egui::ComboBox::from_id_salt(id)
            .width(260.0)
            .selected_text(egui::RichText::new(current).size(14.0).strong())
            .show_ui(u, |u| {
                for (i, name) in names.iter().enumerate() {
                    if u.selectable_value(&mut self.set.active, i, name.clone())
                        .changed()
                    {
                        switched = true;
                    }
                }
            });
        if unnamed && warn_unnamed {
            ui::warn(u, "фамилия не заполнена — в настройках");
        }
        if switched {
            // Печать и подпись — файлы нового врача. Оставить прежние картинки
            // значит заверить документ чужим клише.
            self.recut_ink();
            self.confirm_delete = false;
            self.status = Status::Ok(format!("Подписывает {}", self.set.doc().title()));
        }
    }

    /// Заголовок окна уже в системной раме — здесь только врач и настройки.
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(ui::PAD, 8)),
            )
            .show(ctx, |u| {
                u.horizontal(|u| {
                    self.doctor_picker(u, "top_doctor", true);
                    // Высоту ряда берём ту, что уже набрал выбор врача, и
                    // отдаём её правой части. Без этого `with_layout` получает
                    // всю оставшуюся высоту панели, считает её своим рядом и
                    // прижимает кнопку к верхнему краю: выбор врача выше,
                    // кнопка «Настройки» ниже ростом — и низ у них разъезжается.
                    // `Align::Center` тогда не спасает, потому что центрует
                    // кнопку внутри её собственного, а не общего ряда.
                    let h = u.min_size().y;
                    let right = egui::Layout::right_to_left(egui::Align::Center);
                    let space = egui::vec2(u.available_width(), h);
                    u.allocate_ui_with_layout(space, right, |u| {
                        // И той же высоты: выбор врача набран крупнее (14 pt
                        // полужирным против 12.5 у кнопки), поэтому сам по себе
                        // он на пару пикселей выше — низ у соседей разъезжается,
                        // и ряд выглядит собранным наспех.
                        let b = egui::Button::new("Настройки").min_size(egui::vec2(0.0, h));
                        if u.add(b).clicked() {
                            self.set_backup = Some(self.set.clone());
                            self.confirm_delete = false;
                            self.show_settings = true;
                        }
                    });
                });
                u.add_space(6.0);
                u.horizontal(|u| {
                    let pages: usize = package::estimate(&self.sources, self.quality).0;
                    u.selectable_value(&mut self.tab, Tab::Vypiska, "  1. Выписка  ");
                    u.selectable_value(
                        &mut self.tab,
                        Tab::Package,
                        if pages == 0 {
                            "  2. Пакет  ".to_string()
                        } else {
                            format!("  2. Пакет — {pages} стр.  ")
                        },
                    );
                });
            });
    }

    fn bottom_bar(&mut self, ctx: &egui::Context, problems: &[String]) {
        egui::TopBottomPanel::bottom("bottom")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(ui::PAD, 10)),
            )
            .show(ctx, |u| {
                // Авторство стоит в одном ряду с кнопкой, у правого края.
                // Отдельной строкой под кнопкой оно оставляло внизу пустую
                // полосу — панель росла вдвое ради одной мелкой надписи.
                //
                // Порядок здесь обратный: раскладка справа налево, поэтому
                // надпись добавляется ПЕРВОЙ и встаёт у правого края, а кнопка
                // с сообщениями занимает всё, что осталось слева.
                u.with_layout(egui::Layout::right_to_left(egui::Align::Center), |u| {
                    // Кликабельна: за ней спрятано окно «О программе» с
                    // условиями использования. Отдельной кнопки не даём —
                    // врачу она не нужна ни разу за приём. Но раз уж кликается,
                    // это должно быть видно ДО щелчка: под курсором надпись
                    // светлеет и подчёркивается, курсор становится рукой.
                    // Всплывающая подсказка одна с этим не справляется — её
                    // ещё надо дождаться, и до неё строка выглядит мёртвой.
                    //
                    // Рисуем руками, а не `Button`: у кнопки без рамки цвет
                    // текста задан заранее и на наведение не отзывается, а с
                    // рамкой в углу появился бы полноценный прямоугольник,
                    // спорящий с «Сохранить выписку». Кликабельная надпись
                    // (`Label::sense`) тоже отпадает: в этом месте у неё
                    // выходила нулевая область нажатия — текст рисовался,
                    // щелчок не проходил, поймано тестом
                    // `the_author_line_opens_the_terms`.
                    //
                    // Высота 38 — не украшательство: в горизонтальном ряду
                    // egui не перецентровывает то, что добавлено раньше более
                    // высокого соседа, и без этого надпись повисла бы у
                    // верхнего края. Заодно это нормальная площадь попадания.
                    let line = format!("{} · {}", crate::АВТОР, env!("CARGO_PKG_VERSION"));
                    // PLACEHOLDER — чтобы цвет можно было выбрать после того,
                    // как станет известно наведение: раскладка уже посчитана,
                    // а краска ещё нет.
                    let galley = u.painter().layout_no_wrap(
                        line,
                        egui::FontId::proportional(10.5),
                        egui::Color32::PLACEHOLDER,
                    );
                    let (rect, resp) = u.allocate_exact_size(
                        egui::vec2(galley.size().x, 38.0),
                        egui::Sense::click(),
                    );
                    let hovered = resp.hovered();
                    let color = if hovered {
                        u.visuals().strong_text_color()
                    } else {
                        ui::MUTED
                    };
                    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
                    if hovered {
                        let y = pos.y + galley.size().y - 1.0;
                        u.painter().line_segment(
                            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                            egui::Stroke::new(1.0_f32, color),
                        );
                    }
                    u.painter().galley(pos, galley, color);
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("О программе")
                        .clicked()
                    {
                        self.show_about = true;
                    }
                    u.add_space(14.0);

                    // Раскладка задаётся ЯВНО, слева направо. `horizontal_wrapped`
                    // здесь не годится: он наследует направление у родителя, а
                    // родитель у нас справа налево — и ряд переворачивался,
                    // кнопка уезжала вправо, сообщение вставало слева от неё.
                    //
                    // Перенос оставлен: к кнопке иногда встают сразу две строки —
                    // «Сохранено …» и напоминание про незагруженную печать, — и
                    // без переноса вторая просто обрезается по краю окна.
                    let row = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
                    let space = u.available_size();
                    u.allocate_ui_with_layout(space, row, |u| {
                        let package_tab = self.tab == Tab::Package;
                        let ready = if package_tab {
                            !self.sources.is_empty()
                        } else {
                            problems.is_empty()
                        };
                        let title = if package_tab {
                            "Собрать пакет"
                        } else {
                            "Сохранить выписку"
                        };
                        let btn = egui::Button::new(egui::RichText::new(title).size(15.0))
                            .min_size(egui::vec2(190.0, 38.0));
                        if u.add_enabled(ready, btn).clicked() {
                            if package_tab {
                                self.build_package();
                            } else {
                                self.save();
                            }
                        }
                        // Печать отдельной кнопкой, а не «сохраните и
                        // напечатайте из просмотрщика»: тот путь держится на
                        // трёх «если» — просмотрщик есть, он умеет печатать,
                        // врач знает про Ctrl+P. Не совпало одно — документ
                        // есть, бумаги нет.
                        if !package_tab {
                            u.add_space(8.0);
                            let p = egui::Button::new(egui::RichText::new("Печать").size(15.0))
                                .min_size(egui::vec2(120.0, 38.0));
                            if u.add_enabled(ready, p).clicked() {
                                self.print_vypiska();
                            }
                        }
                        u.add_space(14.0);
                        // Одной строкой в том же ряду: вложенный столбец прижимал
                        // текст к верхнему краю кнопки, и надпись висела криво.
                        match (&self.status, ready) {
                            (Status::Err(e), _) => ui::warn(u, e),
                            (_, false) if package_tab => {
                                ui::warn(u, "Добавьте хотя бы одну страницу")
                            }
                            (_, false) if self.nap.is_none() => {
                                // Без направления ФИО и дату рождения вписать негде —
                                // перечислять их значит требовать невозможного.
                                ui::warn(u, "Сначала загрузите направление из КУМки")
                            }
                            (_, false) => {
                                ui::warn(u, &format!("не хватает: {}", problems.join(", ")))
                            }
                            (Status::Ok(s), true) => {
                                u.label(egui::RichText::new(s).size(11.5).color(ui::OK));
                            }
                            (Status::Idle, true) => {}
                        }
                        // Пока документ ещё не готов, внизу и так висит список
                        // недостающего — вторая красная строка рядом с ним только
                        // растворила бы обе. Говорим ровно в тот момент, когда
                        // остаётся одно действие: нажать «Сохранить выписку».
                        if !package_tab && ready {
                            if let Some(msg) = self.ink_not_loaded() {
                                ui::warn(u, &msg);
                            }
                        }
                    });
                });
            });
    }

    fn body(&mut self, u: &mut egui::Ui, hovering: bool) {
        match self.tab {
            Tab::Vypiska => self.body_vypiska(u, hovering),
            Tab::Package => self.body_package(u, hovering),
        }
    }

    fn body_vypiska(&mut self, u: &mut egui::Ui, hovering: bool) {
        self.drop_zone(u, hovering);

        if let Some(n) = &self.nap {
            let fio = n.fio.clone();
            let dob = fmt::dob_full(&n.dob, fmt::today());
            // Адрес в сводке не показываем: он ниже отдельным полем, которое
            // врач правит. Две строки с адресом, одна из которых устарела,
            // хуже одной — непонятно, какая уйдёт в документ.
            let line = format!("Полис {}", n.policy);
            ui::section(u, "Пациент");
            u.label(egui::RichText::new(fio).size(16.0).strong());
            if dob.is_empty() {
                ui::warn(u, "дату рождения не удалось прочитать из направления");
            } else {
                u.label(dob);
            }
            ui::hint(u, &line);
            u.add_space(ui::GAP_ROW);
        }

        // Осмотр идёт раньше документов: он обязательный, а СНИЛС и телефон —
        // нет, и прятать обязательное под необязательным неправильно.
        ui::section(u, "Осмотр");
        ui::grid(u, "exam", |u| {
            ui::label_required(u, "Диагноз основной");
            ui::edit_multi_hint(
                u,
                &mut self.pat.diagnosis,
                2,
                "J06.9 Острая инфекция верхних дыхательных путей",
            );
            u.end_row();
            ui::label_required(u, "Жалобы");
            ui::edit_hint_fill(u, &mut self.pat.complaints, "активно не предъявляет");
            u.end_row();
            ui::label_required(u, "ЧДД, в мин");
            ui::edit_digits(u, &mut self.pat.chdd, "", crate::mask::vital);
            u.end_row();
            ui::label_required(u, "ЧСС, в мин");
            ui::edit_digits(u, &mut self.pat.chss, "", crate::mask::vital);
            u.end_row();
        });
        ui::hint(u, "В направлении этих цифр нет — впишите из своего осмотра");
        self.age_norm_hint(u);

        ui::section(u, "Документы пациента");
        let mut snils_bad = false;
        ui::grid(u, "docs", |u| {
            // Адрес правится руками: из КУМки приходит регистрация, а в
            // документе нужно место, где ребёнок живёт на самом деле.
            ui::label(u, "Адрес проживания");
            ui::edit_hint(u, &mut self.pat.address, "город, улица, дом, квартира");
            u.end_row();

            ui::label(u, "СНИЛС");
            // Врач набирает только цифры: дефисы и пробел ставятся сами,
            // стереть их нельзя, буквы в значение не попадают.
            ui::edit_digits(u, &mut self.pat.snils, "___-___-___ __", crate::mask::snils);
            snils_bad = crate::mask::snils_ok(&self.pat.snils) == Some(false);
            u.end_row();

            ui::label(u, "Телефон");
            ui::edit_digits(
                u,
                &mut self.pat.phone,
                "+7 (___)-___-__-__",
                crate::mask::phone,
            );
            u.end_row();

            ui::label(u, "Свидетельство о рождении");
            // Здесь не только цифры — серия римская и буквенная.
            if ui::edit_hint(u, &mut self.pat.birth_cert, "IV-БА ______").lost_focus() {
                self.pat.birth_cert = crate::mask::birth_cert(&self.pat.birth_cert);
            }
            u.end_row();

            ui::label(u, "Организованность");
            ui::edit_hint(u, &mut self.pat.organized, "школа или детский сад");
            u.end_row();
        });
        // Как только СНИЛС дописан, смотрим, был ли этот ребёнок раньше.
        let digits = self
            .pat
            .snils
            .chars()
            .filter(|c| c.is_ascii_digit())
            .count();
        if digits == 11 {
            if self
                .found_card
                .as_ref()
                .map(|c| c.snils != self.pat.snils)
                .unwrap_or(true)
            {
                match store::load(&self.pat.snils) {
                    Ok(card) => {
                        self.found_card = card;
                        self.card_err = None;
                    }
                    // Карточка есть, но не открылась: чаще всего флешку принесли
                    // на чужой компьютер, где DPAPI её не расшифрует. Промолчать —
                    // значит сказать врачу, что ребёнок новый, и похоронить
                    // накопленный анамнез под пустой карточкой.
                    Err(e) => {
                        self.found_card = None;
                        self.card_err = Some(format!("{e:#}"));
                    }
                }
            }
        } else {
            self.found_card = None;
            self.card_err = None;
        }
        if let Some(msg) = self.card_err.clone() {
            ui::warn(u, &format!("Карточка этого ребёнка не читается: {msg}"));
        }
        if let Some(card) = self.found_card.clone() {
            u.horizontal_wrapped(|u| {
                ui::hint(
                    u,
                    &format!("Этот ребёнок уже был: карточка от {}.", card.updated),
                );
                if u.button("Подставить прошлый визит").clicked() {
                    self.apply_card(card);
                }
            });
        }
        if snils_bad {
            // Не блокируем: номер бывает старый, без контрольного числа.
            // Но опечатку лучше поймать до того, как документ ушёл.
            ui::warn(
                u,
                "СНИЛС не сходится по контрольному числу — проверьте цифры",
            );
        }
        ui::hint(u, "детский сад, школа или «неорганизован»");

        // Код повода из направления — только подсказка. Подставлять его в графу
        // «Диагноз основной» нельзя: у тестового пациента направление выписано с Z13.8,
        // а в выписке врач указывает клинический диагноз.
        if let Some(hint) = self.nap.as_ref().map(|n| n.diagnosis.clone()) {
            if !hint.is_empty() && hint != self.pat.diagnosis {
                u.horizontal_wrapped(|u| {
                    let code = hint.split_whitespace().next().unwrap_or(&hint).to_string();
                    ui::hint(
                        u,
                        &format!("В направлении повод: {code}. Это не диагноз ребёнка."),
                    );
                    if u.button("Подставить целиком").clicked() {
                        self.pat.diagnosis = hint.clone();
                    }
                });
            }
        }

        ui::section(u, "Анамнез заболевания");
        ui::edit_multi_hint(
            u,
            &mut self.pat.anamnesis_disease,
            6,
            "С какого числа болен, что было, чем лечили, результаты обследований",
        );

        u.add_space(ui::GAP_SECTION);
        self.exam_template_block(u);
        self.extra_block(u);
        u.add_space(ui::GAP_SECTION);
    }

    /// Добавить снимок или PDF в пакет.
    /// Куда девать бро́шенный в окно файл. Решает открытая вкладка.
    ///
    /// Раньше решал только тип файла: всякий .odt считался направлением и шёл
    /// в разбор полей. Из КУМки в .odt выгружается не только направление, но
    /// и обследования, и врач бросает их в пакет — а программа пыталась
    /// прочитать в них ФИО, не находила, отвечала «это не похоже на
    /// направление» и заодно выгружала уже загруженное направление. Её слова:
    /// «когда я хочу собрать пакет, он видит только файлы пдф, а надо ещё
    /// одт».
    ///
    /// Вкладка отвечает на этот вопрос точнее типа файла: на «1. Выписка» она
    /// работает с направлением этого ребёнка, на «2. Пакет» — складывает
    /// документы. Поэтому на первой .odt и заполняет поля, и ложится в пакет,
    /// а на второй — только ложится в пакет, ничего не трогая.
    fn accept_dropped(&mut self, path: &std::path::Path) {
        if is_odt(path) && self.tab == Tab::Vypiska {
            // В пакет — только если это правда направление. Не разобралось
            // (бросили обследование не на ту вкладку) — в пакете ему не место
            // первой страницей, а причину уже сказал разбор.
            if self.open_odt(path) {
                self.queue_source(path, package::Kind::Referral);
            }
        } else {
            self.queue_source(path, package::Kind::Attachment);
        }
    }

    /// Направление выписал не тот врач, что подписывает сейчас?
    ///
    /// Печать и подпись ставятся из активного профиля, и под направлением
    /// другого врача они стали бы подписью одного человека под фамилией
    /// другого. Сверяем по фамилии: инициалы в КУМке и в профиле пишут
    /// по-разному («У. Д.» и «У.Д.»). Не с чем сверить — не мешаем.
    fn referral_by_someone_else(&self) -> Option<String> {
        let issued_by = self.nap.as_ref().map(|n| n.doctor.as_str()).unwrap_or("");
        let signs = self.set.doc().name.as_str();
        let surname = |s: &str| s.split_whitespace().next().unwrap_or("").to_lowercase();
        (!issued_by.trim().is_empty()
            && !signs.trim().is_empty()
            && surname(issued_by) != surname(signs))
        .then(|| {
            format!(
                "направление выписал(а) {issued_by}, а подписывает {signs} — чужую подпись под ним не ставим"
            )
        })
    }

    /// Поставить файл в очередь на добавление и сказать об этом вслух.
    fn queue_source(&mut self, path: &std::path::Path, kind: package::Kind) {
        self.status = Status::Ok(if is_odt(path) {
            // Честно называем причину паузы: без этого секундная заминка на
            // запуске LibreOffice выглядит как зависшая программа.
            "Перевожу документ из КУМки в PDF для пакета — это пара секунд…".into()
        } else {
            "Добавляю в пакет…".into()
        });
        self.pending_source.push((path.to_path_buf(), kind));
    }

    /// Добавить документ в пакет.
    ///
    /// Читает файл не сама, а через `package::source_from_file`: там и разбор
    /// типа, и перевод направления из .odt в PDF, и внятный отказ, когда
    /// перевести нечем. Своё `fs::read` здесь было бы вторым входом в пакет
    /// мимо этих проверок — и направление снова попало бы в список
    /// неразобранным.
    fn add_source_from(&mut self, path: &std::path::Path, kind: package::Kind) {
        // Печать и подпись ставятся и на направление, а не только на выписку:
        // «На направлении тоже печать надо, чтоб была, и подпись, а то только
        // на выписке». Ставятся тем же выключателем, что и на выписке, — одна
        // галочка на весь документооборот.
        let mut ink_warning = None;
        let someone_else = self.referral_by_someone_else();
        if kind == package::Kind::Referral && self.set.overlay {
            ink_warning.clone_from(&someone_else);
        }
        let someone_else = someone_else.is_some();
        let made = if kind == package::Kind::Referral
            && self.set.overlay
            && !someone_else
            && (self.stamp_png.is_some() || self.sign_png.is_some())
        {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            std::fs::read(path)
                .with_context(|| format!("не прочитать {}", path.display()))
                .and_then(|odt| {
                    let inked = match crate::odt::with_ink(
                        &odt,
                        self.stamp_png.as_deref(),
                        self.set.stamp_cm,
                        self.sign_png.as_deref(),
                    ) {
                        Ok(b) => b,
                        // Направление без оттиска всё равно нужно в пакете —
                        // но врач обязана узнать об этом до отправки.
                        Err(e) => {
                            ink_warning = Some(format!("{e:#}"));
                            odt
                        }
                    };
                    package::source_from_bytes(name, inked, kind)
                })
        } else {
            package::source_from_file(path, kind)
        };
        match made {
            Ok(src) => {
                // Один и тот же документ дважды в пакете не нужен: врач
                // бросает направление мышкой, а потом жмёт «Выбрать файл…» —
                // и обе дороги ведут сюда.
                self.sources.retain(|s| s.name != src.name);
                // Направление в пакете одно. Загрузили исправленное под
                // другим именем — прежнее уходит, а не встаёт второй страницей.
                if src.kind == package::Kind::Referral {
                    self.sources.retain(|s| s.kind != package::Kind::Referral);
                }
                self.sources.push(src);
                // Порядок, который назвала врач: направление, выписка,
                // обследования. Раскладываем на каждом добавлении, а не
                // только перед сборкой: список на экране — это и есть то,
                // что она проверяет глазами перед отправкой.
                package::in_order(&mut self.sources);
                self.status = match ink_warning {
                    Some(w) => Status::Err(format!("Направление в пакете, но без печати: {w}")),
                    None => Status::Ok(format!("В пакете {} стр.", self.page_count())),
                };
            }
            Err(e) => self.status = Status::Err(format!("{e:#}")),
        }
    }

    fn page_count(&self) -> usize {
        package::estimate(&self.sources, self.quality).0
    }

    /// Забрать то, что пришло с телефона, и положить в пакет.
    fn drain_intake(&mut self) {
        let Some(rx) = &self.intake else { return };
        let got = rx.take();
        if got.is_empty() {
            return;
        }
        for r in got {
            self.sources.push(package::Source {
                name: r.name,
                bytes: r.bytes,
                kind: package::Kind::Attachment,
            });
        }
        self.status = Status::Ok(format!("В пакете {} стр.", self.page_count()));
    }

    fn build_package(&mut self) {
        let name = self
            .nap
            .as_ref()
            .and_then(|n| n.fio.split_whitespace().next().map(|s| s.to_string()))
            .unwrap_or_else(|| "пакет".into());
        let Some((path, auto)) = ask_where_to_save(&format!("{name} пакет.pdf")) else {
            return;
        };
        // Целевой размер: принимающая сторона не открывает тяжёлые файлы.
        match package::build(&self.sources, self.quality, Some(3_500_000)).and_then(|pdf| {
            std::fs::write(&path, &pdf)?;
            Ok(pdf.len())
        }) {
            Ok(n) => {
                self.status = Status::Ok(format!(
                    "Пакет собран — {:.1} МБ: {}{}",
                    n as f64 / 1_048_576.0,
                    path.display(),
                    if auto {
                        " (окно выбора папки система не показала — файл положен сюда)"
                    } else {
                        ""
                    }
                ));
                let _ = open_in_viewer(&path);
            }
            Err(e) => self.status = Status::Err(format!("{e:#}")),
        }
    }

    /// Сохранить карточку ребёнка, чтобы повторный визит собирался за минуту.
    fn remember_card(&mut self) {
        let p = &self.pat;
        if p.snils.chars().filter(|c| c.is_ascii_digit()).count() != 11 {
            return; // без СНИЛС карточку не к чему привязать
        }
        let card = store::Card {
            snils: p.snils.clone(),
            fio: self.nap.as_ref().map(|n| n.fio.clone()).unwrap_or_default(),
            phone: p.phone.clone(),
            birth_cert: p.birth_cert.clone(),
            organized: p.organized.clone(),
            diagnosis: p.diagnosis.clone(),
            anamnesis_life: p.anamnesis_life.clone(),
            anamnesis_disease: p.anamnesis_disease.clone(),
            updated: String::new(),
        };
        // Отказ сохранения нельзя глотать: карточка теперь шифруется, и
        // сохранение может не удаться по-настоящему (нет ключа, нет доступа
        // к домашней папке, сетевой каталог отвалился). Молчание здесь значит,
        // что врач уверена: анамнез записан, — а на следующем визите его нет.
        if let Err(e) = store::save(&card) {
            self.card_err = Some(format!("карточка не сохранена: {e:#}"));
        }
    }

    /// Подставить всё, что помнит карточка, кроме осмотра: жалобы, ЧДД и ЧСС
    /// у каждого визита свои, и тянуть их из прошлого раза нельзя.
    fn apply_card(&mut self, c: store::Card) {
        self.pat.phone = c.phone;
        self.pat.birth_cert = c.birth_cert;
        self.pat.organized = c.organized;
        self.pat.diagnosis = c.diagnosis;
        self.pat.anamnesis_life = c.anamnesis_life;
        self.pat.anamnesis_disease = c.anamnesis_disease;
        self.found_card = None;
        self.status = Status::Ok("Подставлено из прошлого визита".into());
    }

    /// Подсказка, не запрет: подозрительные ЧДД/ЧСС не мешают сохранить
    /// документ, только предупреждают. Молчит, пока дата рождения не
    /// разобралась или возраст вышел за пределы таблицы (`norms.rs`) — там,
    /// где у источника нет данных, лучше ничего не сказать, чем угадать.
    fn age_norm_hint(&self, u: &mut egui::Ui) {
        let Some(n) = &self.nap else { return };
        let Some(date) = fmt::to_date(&n.dob) else {
            return;
        };
        let Some(months) = fmt::age_months(date, fmt::today()) else {
            return;
        };
        if let Ok(v) = self.pat.chdd.trim().parse::<u32>() {
            if let Some((lo, hi)) = norms::suspicious_chdd(months, v) {
                ui::warn(
                    u,
                    &format!("ЧДД {v} — обычный диапазон для этого возраста {lo}–{hi}, проверьте"),
                );
            }
        }
        if let Ok(v) = self.pat.chss.trim().parse::<u32>() {
            if let Some((lo, hi)) = norms::suspicious_chss(months, v) {
                ui::warn(
                    u,
                    &format!("ЧСС {v} — обычный диапазон для этого возраста {lo}–{hi}, проверьте"),
                );
            }
        }
    }

    fn body_package(&mut self, u: &mut egui::Ui, hovering: bool) {
        ui::section(u, "Снимки с телефона");
        match &self.intake {
            None => {
                ui::hint(
                    u,
                    "Программа покажет код со своим адресом в сети. Наведёте камеру телефона — снимки придут сюда, без ВК.",
                );
                u.add_space(ui::GAP_ROW);
                if u.button("Показать код для телефона").clicked() {
                    match intake::Intake::start() {
                        Ok(rx) => {
                            self.qr = None;
                            self.intake = Some(rx);
                        }
                        Err(e) => self.status = Status::Err(format!("{e:#}")),
                    }
                }
            }
            Some(rx) => {
                let url = rx.url().to_string();
                let count = rx.count();
                let addrs = rx.addresses().to_vec();
                let current = rx.ip();
                let hits = rx.hits();
                // Копию делаем потому, что `rx` взят на чтение, а рисовать
                // надо внутри замыкания, которое трогает `self`.
                let checked = rx.last_checkup().map(|c| {
                    (
                        c.ok,
                        c.text.clone(),
                        c.command.clone(),
                        c.command_note.clone(),
                    )
                });
                // Картинку кода готовим один раз: она не меняется, пока адрес тот же.
                if self.qr.is_none() {
                    if let Ok(png) = rx.qr_png() {
                        if let Ok(img) = image::load_from_memory(&png) {
                            let rgba = img.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let tex = u.ctx().load_texture(
                                "qr",
                                egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                                egui::TextureOptions::NEAREST,
                            );
                            self.qr = Some(tex);
                        }
                    }
                }
                // Что нажали — разбираем после отрисовки: пока рисуем, приём
                // взят на чтение, и менять адрес прямо в замыкании нельзя.
                let mut pick = None;
                let mut recheck = false;
                let mut stop = false;
                u.horizontal(|u| {
                    if let Some(tex) = &self.qr {
                        u.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(150.0, 150.0)));
                    }
                    u.add_space(12.0);
                    u.vertical(|u| {
                        u.label(egui::RichText::new("Наведите камеру телефона").size(14.0));
                        ui::hint(u, &url);
                        // Список только когда выбирать действительно есть из
                        // чего: при одном адресе он был бы лишним вопросом.
                        if addrs.len() > 1 {
                            ui::hint(u, "Компьютер в нескольких сетях. Если телефон не открыл — выберите другой адрес:");
                            u.horizontal_wrapped(|u| {
                                for (name, ip) in &addrs {
                                    // Точка, а не только заливка: какой адрес
                                    // выбран, должно быть видно и без цвета.
                                    let mark = if *ip == current { "• " } else { "" };
                                    let label = if intake::is_hotspot(ip) {
                                        format!("{mark}{ip} — раздача с телефона")
                                    } else {
                                        format!("{mark}{ip} — {name}")
                                    };
                                    let b = egui::Button::new(label)
                                        .selected(*ip == current)
                                        .min_size(egui::vec2(0.0, 26.0));
                                    if u.add(b).clicked() {
                                        pick = Some(*ip);
                                    }
                                }
                            });
                        }
                        u.add_space(ui::GAP_ROW);
                        u.label(format!("Принято снимков: {count}"));
                        // Число обращений рядом со снимками — это тот самый
                        // различитель, ради которого всё затевалось: ноль
                        // значит «телефон до компьютера не доходит», а не ноль
                        // при нуле снимков — «дошёл, но отправку не нажали».
                        // Врач видит, как оно меняется, ещё до кнопки проверки.
                        ui::hint(u, &format!("обращений с телефона: {hits}"));
                        // Итог самопроверки — здесь, а не в строке внизу: пока
                        // в пакете нет страниц, та строка занята напоминанием
                        // их добавить, и ответ проверки врач бы не увидел.
                        //
                        // Цвет по двум признакам, а не по одному: `ok` значит
                        // всего лишь «сервер отвечает по адресу из QR». При
                        // закрытом брандмауэре он отвечает — сам себе, — и
                        // зелёный на такой ответ означал бы «всё хорошо» там,
                        // где снимки не идут. Есть команда для админа — это
                        // предупреждение, а не успех.
                        if let Some((ok, text, command, note)) = &checked {
                            let color = match (ok, command.is_some()) {
                                (false, _) => ui::WARN,
                                (true, true) => ui::ATTENTION,
                                (true, false) => ui::OK,
                            };
                            // Переносы в тексте — это абзацы и нумерованный
                            // список, их нельзя схлопывать в одну строку.
                            u.label(egui::RichText::new(text).size(11.5).color(color));
                            if let Some(cmd) = command {
                                u.add_space(4.0);
                                u.horizontal(|u| {
                                    // Поле, а не надпись: команду копируют
                                    // мышкой, а не переписывают с экрана.
                                    let mut shown = cmd.clone();
                                    u.add(
                                        egui::TextEdit::singleline(&mut shown)
                                            .font(egui::TextStyle::Monospace)
                                            .desired_width(430.0),
                                    );
                                    if u.button("Скопировать").clicked() {
                                        u.ctx().copy_text(cmd.clone());
                                    }
                                });
                                if let Some(n) = note {
                                    ui::hint(u, n);
                                }
                            }
                        }
                        // Белая страница на телефоне — это НЕ «сайт открылся».
                        // Safari рисует белый лист всё время, пока пакеты не
                        // доходят, и только через минуту с лишним показывает
                        // ошибку. Врач столько не ждёт и решает, что сломалось
                        // у нас, — поэтому объясняем прямо.
                        ui::hint(u, "Белая страница на телефоне — он не дошёл до компьютера, программа тут ни при чём. Нажмите «Проверить приём».");
                        // Запасной путь висит всегда, а не появляется после
                        // неудачи: на него ссылается каждый разбор, и он
                        // единственный, который работает при любой сети.
                        ui::hint(u, intake::FALLBACK);
                        u.add_space(ui::GAP_ROW);
                        u.horizontal(|u| {
                            if u.button("Проверить приём").clicked() {
                                recheck = true;
                            }
                            if u.button("Больше не принимать").clicked() {
                                stop = true;
                            }
                        });
                    });
                });
                if let Some(ip) = pick {
                    if let Some(rx) = &mut self.intake {
                        rx.use_address(ip);
                        self.qr = None;
                    }
                }
                if recheck {
                    if let Some(rx) = &mut self.intake {
                        rx.check();
                        // Проверка могла переехать на другой адрес — код
                        // перерисуем, а ответ показываем следующим кадром.
                        self.qr = None;
                        u.ctx().request_repaint();
                    }
                }
                if stop {
                    self.intake = None;
                    self.qr = None;
                }
            }
        }

        ui::section(u, "Страницы пакета");
        // Кнопка рядом с перетаскиванием, а не вместо него. Перетаскивание —
        // её обычный путь, но выгрузка из КУМки лежит в своей папке, и
        // таскать оттуда мышкой через два открытых окна неудобно.
        u.horizontal(|u| {
            if u.button("Добавить обследования…").clicked() {
                if dialogs_work() {
                    if let Some(paths) = rfd::FileDialog::new()
                        // Одним списком все типы: врач не обязана помнить, что
                        // из КУМки приходит .odt, а со сканера .pdf.
                        .add_filter(
                            "Документы и снимки",
                            &["pdf", "odt", "jpg", "jpeg", "png", "heic", "tif", "tiff"],
                        )
                        .pick_files()
                    {
                        for p in paths {
                            // Кнопка живёт на вкладке «Пакет», и всё, что
                            // через неё приходит, — обследования: направление
                            // загружается на первой вкладке, выписка родится
                            // сама.
                            self.queue_source(&p, package::Kind::Attachment);
                        }
                    }
                } else {
                    self.status = Status::Err(NO_DIALOGS.into());
                }
            }
            ui::hint(u, "УЗИ, анализы, заключения");
        });
        u.add_space(ui::GAP_ROW);
        if self.sources.is_empty() {
            // Три коротких строки вместо одного длинного абзаца: это место
            // читают мельком, между пациентами, и сплошной текст тут
            // пролистывают не читая. Каждая строка отвечает на свой вопрос:
            // что делать, что подойдёт, откуда возьмётся выписка.
            if hovering {
                u.label(egui::RichText::new("Отпустите файлы здесь").strong());
            } else {
                u.label(egui::RichText::new("Пока пусто.").strong());
                u.add_space(2.0);
                u.label("Перетащите обследования сюда — или нажмите «Добавить обследования…».");
                ui::hint(u, "Подойдут .odt из КУМки, PDF и снимки.");
                ui::hint(u, "Выписка ляжет сюда сама, когда вы её сохраните.");
            }
        } else {
            let mut remove = None;
            let mut swap = None;
            let n = self.sources.len();
            for (i, src) in self.sources.iter().enumerate() {
                u.horizontal(|u| {
                    u.label(egui::RichText::new(format!("{}.", i + 1)).color(ui::MUTED));
                    u.label(&src.name);
                    u.with_layout(egui::Layout::right_to_left(egui::Align::Center), |u| {
                        if u.small_button("убрать").clicked() {
                            remove = Some(i);
                        }
                        if i + 1 < n && u.small_button("вниз").clicked() {
                            swap = Some((i, i + 1));
                        }
                        if i > 0 && u.small_button("вверх").clicked() {
                            swap = Some((i, i - 1));
                        }
                    });
                });
            }
            if let Some(i) = remove {
                self.sources.remove(i);
            }
            if let Some((a, b)) = swap {
                self.sources.swap(a, b);
            }
            ui::hint(u, "Порядок страниц в пакете — сверху вниз");
        }
        // Самое частое недоумение: «пакет не собирается, только направление
        // даёт». Пакет при этом собирается исправно — просто выписки в нём
        // нет, потому что в пакет она попадает в момент сохранения, а
        // сохранение чем-то заблокировано. Со вкладки «Пакет» причины не
        // видно вовсе: внизу там свои сообщения, про пакет. Поэтому говорим
        // прямо здесь и называем, что именно мешает.
        if !self
            .sources
            .iter()
            .any(|s| s.kind == package::Kind::Vypiska)
        {
            let problems = self.problems();
            // Двумя строками: сначала что не так, отдельно — чего не хватает.
            // Одной строкой список нехваток упирался в край окна и читался
            // как сплошная красная простыня.
            if problems.is_empty() {
                ui::warn(u, "Выписки в пакете ещё нет.");
                ui::hint(
                    u,
                    "Она попадёт сюда, когда на вкладке «1. Выписка» нажмёте «Сохранить выписку».",
                );
            } else {
                ui::warn(u, "Выписки в пакете нет: сохранить её пока нельзя.");
                ui::warn(u, &format!("Не хватает: {}", problems.join(", ")));
                ui::hint(u, "Всё это — на вкладке «1. Выписка».");
            }
        }

        ui::section(u, "Качество");
        u.horizontal(|u| {
            for q in [
                package::Quality::Small,
                package::Quality::Normal,
                package::Quality::Sharp,
            ] {
                u.selectable_value(&mut self.quality, q, q.label());
            }
        });
        let (pages, bytes) = package::estimate(&self.sources, self.quality);
        ui::hint(
            u,
            &format!(
                "{pages} стр., примерно {:.1} МБ. Ниже 1240 px не опускаемся — на меньшей ширине номер сертификата в направлении перестаёт читаться.",
                bytes as f64 / 1_048_576.0
            ),
        );
        u.add_space(ui::GAP_SECTION);
    }

    fn drop_zone(&mut self, u: &mut egui::Ui, hovering: bool) {
        let loaded = self.nap.is_some();
        let accent = egui::Color32::from_rgb(90, 140, 210);
        egui::Frame::group(u.style())
            .fill(if hovering {
                u.visuals().selection.bg_fill.gamma_multiply(0.35)
            } else {
                u.visuals().faint_bg_color
            })
            .stroke(if hovering {
                egui::Stroke::new(2.0_f32, accent)
            } else {
                u.visuals().widgets.noninteractive.bg_stroke
            })
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(u, |u| {
                u.set_width(u.available_width());
                u.horizontal(|u| {
                    if hovering {
                        u.label(
                            egui::RichText::new("Отпустите файл здесь")
                                .size(14.0)
                                .color(accent)
                                .strong(),
                        );
                    } else if loaded {
                        u.label(egui::RichText::new("Направление:").weak());
                        u.label(egui::RichText::new(&self.src_name).strong());
                    } else {
                        u.label(
                            egui::RichText::new(
                                "Перетащите сюда направление из КУМки (.odt) — оно заполнит поля и ляжет первой страницей пакета",
                            )
                                .size(14.0),
                        );
                    }
                    u.with_layout(egui::Layout::right_to_left(egui::Align::Center), |u| {
                        let t = if loaded { "Другой пациент…" } else { "Выбрать файл…" };
                        if u.button(t).clicked() {
                            match rfd::FileDialog::new()
                                .add_filter("Направление", &["odt"])
                                .pick_file()
                            {
                                Some(p) => self.open_path(&p),
                                // Пустой ответ значит либо «Отмена», либо что
                                // окно выбора вообще не показалось. Второе —
                                // не отмена, и молчать о нём нельзя: врач
                                // будет жать кнопку и не понимать, почему
                                // ничего не происходит.
                                None if !dialogs_work() => {
                                    self.status = Status::Err(NO_DIALOGS.into())
                                }
                                None => {}
                            }
                        }
                    });
                });
                if loaded {
                    ui::hint(u, "другое направление очистит поля — это новый ребёнок");
                }
            });
    }

    fn exam_template_block(&mut self, u: &mut egui::Ui) {
        egui::CollapsingHeader::new("Текст осмотра — постоянный, правится один раз")
            .id_salt("exam_tpl")
            .show(u, |u| {
                ui::hint(u, "Печатается в каждой выписке. Правки сохраняются.");
                u.add_space(4.0);
                // 13 строк — не на глаз: EXAM_DEFAULT это 643 символа, при
                // ширине поля в окне настроек (~500px) и рабочем кегле это
                // ~50 знаков в строке, то есть ~13 строк без переноса вручную.
                ui::edit_multi_roomy(u, &mut self.set.exam_template, 13);

                let lost: Vec<&str> = [("{жалобы}", "жалобы"), ("{чдд}", "ЧДД"), ("{чсс}", "ЧСС")]
                    .iter()
                    .filter(|(ph, _)| !self.set.exam_template.contains(ph))
                    .map(|(_, n)| *n)
                    .collect();
                if lost.is_empty() {
                    ui::hint(u, "места {жалобы}, {чдд}, {чсс} на месте");
                } else {
                    ui::warn(
                        u,
                        &format!(
                            "стёрто: {} — верните эти места, иначе значения не попадут в документ",
                            lost.join(", ")
                        ),
                    );
                    // Отдельно — про цифры, вписанные прямо сюда. Так и вышло
                    // на живом приёме: подстановки стёрты, а на их месте
                    // «ЧДД 21 в мин» и «ЧСС 93 в мин» — настоящие цифры этого
                    // ребёнка. Текст постоянный, он один на всех, и эти цифры
                    // уехали бы в выписку следующему. Молчать об этом нельзя:
                    // ошибка выглядит как аккуратно заполненный текст.
                    ui::warn(
                        u,
                        "Если вы вписали сюда цифры своего осмотра — уберите их. Этот текст один на всех детей, и цифры уйдут следующему ребёнку. Цифры вписываются на вкладке «1. Выписка», а сюда они подставятся сами на место {чдд} и {чсс}.",
                    );
                }
                u.add_space(ui::GAP_ROW);
                u.label(egui::RichText::new("Как это будет напечатано").size(12.0).strong());
                let preview = self.build().exam();
                u.label(egui::RichText::new(preview).size(11.5));
                u.add_space(ui::GAP_ROW);
                if u.button("Вернуть исходный текст").clicked() {
                    self.set.exam_template = crate::vypiska::EXAM_DEFAULT.to_string();
                }
            });
    }

    fn extra_block(&mut self, u: &mut egui::Ui) {
        egui::CollapsingHeader::new("Анамнез жизни, направление и приложения")
            .id_salt("extra")
            .show(u, |u| {
                u.label("Анамнез жизни");
                // LIFE_DEFAULT — 292 символа, тот же расчёт даёт ~6 строк.
                ui::edit_multi_roomy(u, &mut self.pat.anamnesis_life, 6);
                u.horizontal(|u| {
                    ui::hint(u, "заготовка подставляется каждому новому ребёнку");
                    if u.small_button("вернуть заготовку").clicked() {
                        self.pat.anamnesis_life = self.set.life_template.clone();
                    }
                });
                u.add_space(ui::GAP_ROW);
                u.label("Направляется на консультацию…");
                ui::edit_multi(u, &mut self.pat.referral_note, 2);
                u.add_space(ui::GAP_ROW);
                u.label("Строка про приложения");
                ui::edit(u, &mut self.pat.attachments);
                ui::hint(u, "очистите, если обследования не прикладываются");
            });
    }

    /// Удаление профиля — в два нажатия и никогда последнего.
    ///
    /// Подтверждение спрашиваем всегда: настройки правят того врача, кем
    /// сейчас подписывают, значит удаляется действующая печать с подписью.
    fn delete_doctor_row(&mut self, u: &mut egui::Ui, recut: &mut bool) {
        if self.confirm_delete {
            ui::warn(
                u,
                &format!(
                    "Удалить {}? Этой печатью и подписью заверяются документы.",
                    self.set.doc().title()
                ),
            );
            u.horizontal(|u| {
                if u.button("Да, удалить").clicked() {
                    self.set.remove_active();
                    self.confirm_delete = false;
                    // Печать удалённого врача не должна пережить его профиль.
                    *recut = true;
                }
                if u.button("Оставить").clicked() {
                    self.confirm_delete = false;
                }
            });
            return;
        }
        let last = self.set.doctors.len() < 2;
        u.horizontal(|u| {
            let btn = egui::Button::new("Удалить врача");
            if u.add_enabled(!last, btn).clicked() {
                self.confirm_delete = true;
            }
            if last {
                // Впритык к кнопке пояснение читалось одним словом с ней —
                // разводим явным отступом, как и в остальных местах окна.
                u.add_space(ui::GAP_ROW);
                ui::hint(u, "последнего удалить нельзя — подписывать станет некем");
            }
        });
    }

    /// «О программе»: кто написал и на каких условиях это здесь работает.
    ///
    /// Врачу оно не нужно ни разу за приём — поэтому спрятано за щелчком по
    /// строке авторства внизу, а не занимает кнопку. Нужно оно в другом
    /// разговоре: когда у учреждения появится вопрос, что это за программа и
    /// кто за неё отвечает.
    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        egui::Window::new("О программе")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(ui::PAD, 14)),
            )
            .show(ctx, |u| {
                u.label(egui::RichText::new("Выписки").size(18.0).strong());
                ui::hint(
                    u,
                    "Выписка формы 027/у из направления и пакет документов одним файлом",
                );
                u.add_space(ui::GAP_ROW);
                u.label(format!("Версия {}", env!("CARGO_PKG_VERSION")));
                u.label(format!("Автор: {}", crate::АВТОР));
                u.label(format!("© 2026 {}", crate::АВТОР));

                ui::section(u, "Условия использования");
                egui::ScrollArea::vertical().max_height(260.0).show(u, |u| {
                    u.label(egui::RichText::new(crate::УСЛОВИЯ).size(12.5));
                });
            });
        self.show_about = open;
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut recut = false;
        // Окно выбора файла не показалось — скажем об этом после отрисовки,
        // внутри замыкания трогать `self.status` нельзя.
        let mut no_dialogs = false;
        let mut close = false;
        egui::Window::new("Настройки врача")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style()).inner_margin(egui::Margin::symmetric(ui::PAD, 14)),
            )
            .show(ctx, |u| {
                egui::ScrollArea::vertical().max_height(420.0).show(u, |u| {
                    ui::grid(u, "set_who", |u| {
                        ui::label(u, "Учреждение");
                        ui::edit(u, &mut self.set.clinic);
                        u.end_row();
                    });
                    ui::hint(u, "Общее для всех врачей — как и текст осмотра");

                    ui::section(u, "Врач");
                    // Список и предупреждение о пустой фамилии — от
                    // `doctor_picker` — раньше стояли в одной строке с кнопкой
                    // «Добавить врача» и читались одной кашей. Кнопка теперь
                    // отдельной строкой, с отступом сверху и снизу.
                    self.doctor_picker(u, "set_doctor", false);
                    u.add_space(ui::GAP_ROW);
                    if u.button("Добавить врача").clicked() {
                        self.set.add_doctor();
                        self.confirm_delete = false;
                        // У нового профиля чернил нет — чужие снимаем сразу.
                        recut = true;
                    }
                    // Кнопка переключает окно на ЧИСТЫЙ профиль, и все поля
                    // ниже разом становятся пустыми. Без этой строчки это
                    // читается как «всё, что я заполнила, слетело», и первое,
                    // что делает врач, — набирает свои данные заново, поверх
                    // второго профиля, оставляя первый безымянным.
                    if self.set.doc().is_blank() {
                        ui::warn(
                            u,
                            if self.set.doctors.len() > 1 {
                                "Это новый пустой профиль. Прежний врач не потерян — он в списке выше."
                            } else {
                                "Профиль ещё пустой — заполните его, новый заводить незачем."
                            },
                        );
                    }
                    u.add_space(ui::GAP_ROW);

                    ui::grid(u, "set_doc", |u| {
                        let d = self.set.doc_mut();
                        ui::label(u, "Фамилия и инициалы");
                        ui::edit(u, &mut d.name);
                        u.end_row();
                        ui::label(u, "Должность");
                        ui::edit(u, &mut d.post);
                        u.end_row();
                        // Графы «заведующий отделением» здесь больше нет:
                        // врач попросила убрать его из выписки, и просить
                        // заполнять поле, которое никуда не печатается, —
                        // значит отнимать время впустую. Разбор заведующего
                        // из направления и само поле в профиле оставлены:
                        // вернуть строку в документ — три строки шаблона.
                    });
                    ui::hint(u, "Фамилия встанет под подписью в выписке");

                    u.add_space(ui::GAP_ROW);
                    self.delete_doctor_row(u, &mut recut);

                    ui::section(u, "Печать и подпись");
                    u.checkbox(&mut self.set.overlay, "Ставить печать и подпись в PDF");
                    ui::hint(u, "Снимите галочку, если документ нужно провести бумагой");
                    u.add_space(ui::GAP_ROW);

                    ui::grid(u, "set_ink", |u| {
                        let d = self.set.doc_mut();
                        for (name, path) in
                            [("Печать", &mut d.stamp_path), ("Подпись", &mut d.sign_path)]
                        {
                            ui::label(u, name);
                            u.with_layout(egui::Layout::right_to_left(egui::Align::Center), |u| {
                                if u.button("Выбрать…").clicked() {
                                    if let Some(p) = rfd::FileDialog::new()
                                        .add_filter(
                                            "Снимок или скан",
                                            &["jpg", "jpeg", "png", "pdf"],
                                        )
                                        .pick_file()
                                    {
                                        *path = p.to_string_lossy().into_owned();
                                        recut = true;
                                    } else if !dialogs_work() {
                                        // Путь можно вписать и руками в поле
                                        // рядом — сообщение должно об этом
                                        // сказать, а не просто отказать.
                                        no_dialogs = true;
                                    }
                                }
                                // Путь правят и руками — тогда картинку тоже надо перечитать.
                                if ui::edit(u, path).lost_focus() {
                                    recut = true;
                                }
                            });
                            u.end_row();
                        }
                    });
                    ui::hint(u, "Подойдёт фото, скан или PDF из сканера айфона — фон уберётся сам");
                    for e in self.ink_errs.clone() {
                        ui::warn(u, &e);
                    }

                    // Размер оттиска настраивается: клише у врачей разные.
                    u.add_space(ui::GAP_ROW);
                    u.horizontal(|u| {
                        u.label("Размер печати");
                        u.add(
                            egui::Slider::new(
                                &mut self.set.stamp_cm,
                                crate::vypiska::STAMP_CM_RANGE,
                            )
                            .suffix(" см")
                            .step_by(0.1),
                        );
                        if u.button("2,9 см").clicked() {
                            self.set.stamp_cm = crate::vypiska::STAMP_CM_DEFAULT;
                        }
                    });
                    ui::hint(u, "Ширина оттиска в готовом документе. Проверяется сохранением выписки: в окне размер не виден.");

                });
                // Кнопки действий вне прокрутки: внутри неё они уезжали
                // вместе с содержимым, и до «Сохранить» надо было доскроллить.
                u.add_space(ui::GAP_ROW);
                u.separator();
                u.add_space(ui::GAP_ROW);
                {
                    let u = &mut *u;
                    u.horizontal(|u| {
                        if u.button("Сохранить").clicked() {
                            match self.set.save() {
                                Ok(()) => {
                                    self.status = Status::Ok("Настройки сохранены".into());
                                    close = true;
                                }
                                Err(e) => self.status = Status::Err(format!("{e:#}")),
                            }
                        }
                        if u.button("Отмена").clicked() {
                            if let Some(b) = self.set_backup.clone() {
                                self.set = b;
                                recut = true;
                            }
                            close = true;
                        }
                        if u.button("Пересчитать печать").clicked() {
                            recut = true;
                        }
                    });
                }
            });
        if no_dialogs {
            self.status = Status::Err(format!("{NO_DIALOGS} Путь можно вписать в поле руками."));
        }
        if recut {
            self.recut_ink();
        }
        if close {
            open = false;
        }
        if !open {
            self.set_backup = None;
            // Незаданный вопрос не должен ждать следующего открытия окна.
            self.confirm_delete = false;
        }
        self.show_settings = open;
    }
}

fn is_odt(p: &std::path::Path) -> bool {
    p.extension()
        .map(|e| e.eq_ignore_ascii_case("odt"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Окно для проверок логики: без контекста egui и без чтения настроек.
    fn bare() -> App {
        App {
            set: Settings::default(),
            nap: None,
            src_name: String::new(),
            pat: Patient::new(crate::vypiska::LIFE_DEFAULT),
            stamp_png: None,
            sign_png: None,
            ink_errs: Vec::new(),
            status: Status::Idle,
            show_settings: false,
            show_about: false,
            set_backup: None,
            confirm_delete: false,
            sized: false,
            ink_ready: true,
            seen: Patient::new(crate::vypiska::LIFE_DEFAULT),
            tab: Tab::Vypiska,
            intake: None,
            qr: None,
            sources: Vec::new(),
            quality: package::Quality::Normal,
            found_card: None,
            card_err: None,
            pending_source: Vec::new(),
        }
    }

    fn sample() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/referral.odt")
    }

    /// Самый опасный сценарий: приём идёт подряд, и данные предыдущего
    /// ребёнка не должны пережить загрузку нового направления.
    #[test]
    fn a_referral_for_another_child_clears_the_examination() {
        let mut a = bare();
        a.open_odt(&sample());
        // Подменяем ФИО в уже загруженном направлении: так тот же файл
        // становится «другим ребёнком», и второй образец не нужен.
        a.nap.as_mut().unwrap().fio = "Прошлый Ребёнок Иванович".into();
        a.pat.snils = "112-233-445 95".into();
        a.pat.chdd = "19".into();
        a.pat.chss = "82".into();
        a.pat.diagnosis = "Z00.1 осмотр".into();
        a.pat.anamnesis_disease = "длинный анамнез прошлого ребёнка".into();

        a.open_odt(&sample());

        assert!(a.nap.is_some(), "направление должно было разобраться");
        assert_eq!(a.pat.snils, "", "СНИЛС прошлого ребёнка остался");
        assert_eq!(a.pat.chdd, "", "ЧДД прошлого ребёнка остался");
        assert_eq!(a.pat.chss, "", "ЧСС прошлого ребёнка остался");
        assert_eq!(a.pat.diagnosis, "", "диагноз прошлого ребёнка остался");
        assert_eq!(
            a.pat.anamnesis_disease, "",
            "анамнез прошлого ребёнка остался"
        );
        assert!(!a.problems().is_empty(), "без осмотра документ не готов");
        match &a.status {
            Status::Ok(t) => assert!(
                t.contains("Поля осмотра очищены"),
                "врачу не сказали, почему поля опустели: {t}"
            ),
            other => panic!("ожидалось сообщение об очистке, пришло {other:?}"),
        }
    }

    /// Выписка из сохранения встаёт за направлением, перед обследованиями.
    /// Раньше она дописывалась в конец: порядок наводился только при
    /// добавлении файлов, а сохранение его не звало.
    #[test]
    fn a_saved_vypiska_lands_between_the_referral_and_the_tests() {
        let mut a = bare();
        a.open_odt(&sample());
        at_work(&mut a);
        for (name, kind) in [
            ("057у.odt", package::Kind::Referral),
            ("узи.odt", package::Kind::Attachment),
        ] {
            a.sources.push(package::Source {
                name: name.into(),
                bytes: b"x".to_vec(),
                kind,
            });
        }
        a.put_vypiska_in_package();
        let kinds: Vec<_> = a.sources.iter().map(|s| s.kind).collect();
        assert_eq!(kinds.first(), Some(&package::Kind::Referral), "{kinds:?}");
        assert_eq!(kinds.last(), Some(&package::Kind::Attachment), "{kinds:?}");
        assert!(
            kinds.contains(&package::Kind::Vypiska),
            "выписка не легла: {kinds:?}"
        );
    }

    /// Подпись и печать берутся из активного профиля. Под направлением
    /// другого врача они стали бы подписью одного человека под фамилией
    /// другого — это не оформление, это подлог.
    #[test]
    fn another_doctors_referral_is_not_signed() {
        let mut a = bare();
        a.open_odt(&sample());
        let issued = a.nap.as_ref().unwrap().doctor.clone();
        assert!(
            !issued.is_empty(),
            "в образце не разобрался врач направления"
        );

        a.set.doctors[0].name = "Лапкова А. А.".into();
        assert!(
            a.referral_by_someone_else().is_some(),
            "чужое направление сочли своим"
        );

        // Тот же врач, но инициалы записаны иначе, чем в КУМке («У. Д.»).
        assert!(
            issued.starts_with("Примерова"),
            "в образце другой врач: {issued}"
        );
        a.set.doctors[0].name = "Примерова П.П.".into();
        assert!(
            a.referral_by_someone_else().is_none(),
            "своё направление сочли чужим из-за записи инициалов"
        );
    }

    /// Пакет не чистился между детьми вовсе. Собери пакет следующему — и в
    /// краевую ушли бы направление, выписка и обследования прошлого ребёнка.
    /// Это опаснее оставшихся полей: страницы пакета видны только на второй
    /// вкладке, и врач их перед отправкой может не пересматривать.
    #[test]
    fn another_child_empties_the_package() {
        let mut a = bare();
        a.open_odt(&sample());
        a.nap.as_mut().unwrap().fio = "Прошлый Ребёнок Иванович".into();
        for (name, kind) in [
            ("прошлое направление.odt", package::Kind::Referral),
            ("выписка, стр. 1", package::Kind::Vypiska),
            ("узи прошлого.odt", package::Kind::Attachment),
        ] {
            a.sources.push(package::Source {
                name: name.into(),
                bytes: b"x".to_vec(),
                kind,
            });
        }

        a.open_odt(&sample());

        assert!(
            a.sources.is_empty(),
            "в пакете остались документы прошлого ребёнка: {:?}",
            a.sources.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    /// А вот тот же ребёнок пакет не теряет: направление часто загружают,
    /// когда обследования уже сложены.
    #[test]
    fn the_same_child_keeps_the_package() {
        let mut a = bare();
        a.sources.push(package::Source {
            name: "узи.odt".into(),
            bytes: b"x".to_vec(),
            kind: package::Kind::Attachment,
        });
        a.open_odt(&sample());
        assert_eq!(a.sources.len(), 1, "обследование этого же ребёнка пропало");
    }

    /// Бросили в окно не направление, а обследование — на первую вкладку по
    /// ошибке. Первой страницей пакета как «направление» оно встать не должно.
    #[test]
    fn a_failed_referral_does_not_enter_the_package() {
        let mut a = bare();
        a.accept_dropped(std::path::Path::new("нет-такого-направления.odt"));
        assert!(
            a.pending_source.is_empty(),
            "неразобранный файл ушёл в пакет направлением"
        );
    }

    /// Врач работает не в том порядке, в каком удобно программе: сначала
    /// осмотр и анамнез, направление — потом. Её слова: «загружаю направление
    /// после заполнения анамнеза, ЧСС, ЧД — всё слетает». Стирать здесь
    /// нечего: до направления в полях только то, что она вписала сама и про
    /// этого же ребёнка.
    #[test]
    fn a_referral_keeps_what_the_doctor_already_typed() {
        let mut a = bare();
        a.pat.chdd = "19".into();
        a.pat.chss = "82".into();
        a.pat.complaints = "активно не предъявляет".into();
        a.pat.anamnesis_disease = "болеет с понедельника".into();
        a.pat.address = "Примерная, д.1".into();

        a.open_odt(&sample());

        assert!(a.nap.is_some(), "направление должно было разобраться");
        assert_eq!(a.pat.chdd, "19", "ЧДД слетело");
        assert_eq!(a.pat.chss, "82", "ЧСС слетело");
        assert_eq!(a.pat.complaints, "активно не предъявляет", "жалобы слетели");
        assert_eq!(
            a.pat.anamnesis_disease, "болеет с понедельника",
            "анамнез слетел"
        );
        assert_eq!(
            a.pat.address, "Примерная, д.1",
            "адрес, вписанный руками, перебит адресом из направления"
        );
    }

    /// Пустой адрес направление всё-таки заполняет: это его работа.
    #[test]
    fn an_empty_address_is_filled_from_the_referral() {
        let mut a = bare();
        a.open_odt(&sample());
        assert!(!a.pat.address.trim().is_empty(), "адрес не подставился");
    }

    /// Фамилия врача, выписавшего направление, не должна подменять того,
    /// кто подписывает выписку.
    #[test]
    fn doctor_is_never_taken_from_the_referral() {
        let mut a = bare();
        a.open_odt(&sample());
        assert_eq!(a.set.doc().name, "", "фамилия подставилась из направления");
        assert!(a.problems().iter().any(|p| p.contains("фамилия врача")));
    }

    /// Заведующий — из направления: он свой у каждого пациента и был
    /// разобран из настоящего документа. Профиль врача не должен его
    /// перекрывать, когда направление уже сказало, кто это.
    #[test]
    fn build_takes_head_of_dept_from_the_referral_first() {
        let mut a = bare();
        a.nap = Some(Napravlenie {
            head_of_dept: "Старшая С. С.".into(),
            ..Default::default()
        });
        a.set.doc_mut().head_of_dept = "Руководитель Р. Р.".into();
        assert_eq!(a.build().head_of_dept, "Старшая С. С.");
    }

    /// Направления нет, либо оно есть, но заведующего в нём разобрать не
    /// удалось — тогда запасной вариант из профиля врача, а не пустая графа.
    #[test]
    fn build_falls_back_to_the_doctor_profile_when_referral_has_no_head_of_dept() {
        let mut a = bare();
        a.set.doc_mut().head_of_dept = "Руководитель Р. Р.".into();
        assert_eq!(
            a.build().head_of_dept,
            "Руководитель Р. Р.",
            "без направления вовсе"
        );

        a.nap = Some(Napravlenie::default());
        assert_eq!(
            a.build().head_of_dept,
            "Руководитель Р. Р.",
            "направление загружено, но заведующего в нём нет"
        );
    }

    /// Смена врача в шапке обязана менять и фамилию под документом:
    /// подписать чужой нельзя, это главное, ради чего профили и заведены.
    #[test]
    fn switching_the_profile_changes_who_signs() {
        let mut a = bare();
        a.set.doc_mut().name = "Примерова П. П.".into();
        a.set.add_doctor();
        a.set.doc_mut().name = "Руководитель Р. Р.".into();
        assert_eq!(a.build().doctor, "Руководитель Р. Р.");

        a.set.active = 0;
        assert_eq!(a.build().doctor, "Примерова П. П.");
    }

    /// Печать указана, но файл не читается — это отказ, а не молчаливый
    /// выпуск неподписанного документа.
    #[test]
    fn unreadable_stamp_blocks_saving() {
        let mut a = bare();
        a.set.overlay = true;
        a.set.doc_mut().stamp_path = "D:/нет-такой-печати.jpg".into();
        a.recut_ink();
        assert!(a.stamp_png.is_none());
        assert!(!a.ink_errs.is_empty(), "ошибка чтения должна быть записана");
        assert!(a.problems().iter().any(|p| p == "печать не прочиталась"));

        // Со снятой галочкой документ идёт без печати осознанно.
        a.set.overlay = false;
        assert!(!a.problems().iter().any(|p| p.contains("печать")));
    }

    // --- живое окно без экрана -------------------------------------------
    //
    // egui умеет считать кадр без видеокарты и отдать список нарисованного.
    // Нажимаем по настоящим координатам надписей, а не зовём методы напрямую:
    // «форма слетела» — это про то, что делает мышь, и проверять это надо так
    // же. Через этот же ход отловлено, что ни одно нажатие в настройках
    // полей пациента не трогает.

    fn frame(
        ctx: &egui::Context,
        a: &mut App,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        paint(ctx, a, events)
            .into_iter()
            .filter_map(|c| match c.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    egui::Rect::from_min_size(t.pos, t.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    /// То же самое, но без потерь: сырые фигуры, как их отдала отрисовка.
    /// Нужно, когда проверяется не текст и не место, а краска, — `frame`
    /// цвет выбрасывает.
    fn paint(
        ctx: &egui::Context,
        a: &mut App,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        // Стиль тот же, что и в живом окне: он задаёт высоту кнопок и
        // отступы, а значит и всю геометрию, которую тут проверяют.
        crate::ui::apply_style(ctx);
        let out = ctx.run(input, |ctx| {
            a.top_bar(ctx);
            let problems = a.problems();
            a.bottom_bar(ctx, &problems);
            egui::CentralPanel::default().show(ctx, |u| {
                egui::ScrollArea::vertical().show(u, |u| a.body(u, false));
            });
            a.settings_window(ctx);
            // ВНИМАНИЕ: этот помощник повторяет тело `update()` вручную.
            // Заводишь новое окно — добавь его и сюда, иначе в тестах оно
            // просто не рисуется, и проверка будет искать то, чего нет.
            a.about_window(ctx);
        });
        out.shapes
    }

    /// Прямоугольник, в котором лежит эта надпись: самая тесная из рамок,
    /// целиком её накрывающих. Так из отрисовки достаётся геометрия виджета,
    /// а не только его текста.
    fn box_of(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Rect> {
        let mut found: Option<egui::Rect> = None;
        for c in shapes {
            if let egui::epaint::Shape::Text(t) = &c.shape {
                if t.galley.text().trim() == text {
                    found = Some(egui::Rect::from_min_size(t.pos, t.galley.size()));
                }
            }
        }
        let inner = found?;
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Rect(r) if r.rect.contains_rect(inner) => Some(r.rect),
                _ => None,
            })
            .min_by(|a, b| a.area().total_cmp(&b.area()))
    }

    /// Каким цветом нарисован этот текст. `None` — такого текста на экране
    /// нет.
    ///
    /// Цвет лежит в двух местах: `override_text_color` — когда его навязали
    /// поверх раскладки, `fallback_color` — когда раскладка оставила выбор
    /// краски на потом (так рисуем строку автора). Смотрим оба, иначе
    /// проверка видит `None` там, где цвет есть.
    fn ink_of(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Color32> {
        shapes.iter().find_map(|c| match &c.shape {
            egui::epaint::Shape::Text(t) if t.galley.text().trim() == text => {
                Some(t.override_text_color.unwrap_or(t.fallback_color))
            }
            _ => None,
        })
    }

    fn at(items: &[(String, egui::Rect)], text: &str) -> egui::Pos2 {
        items
            .iter()
            .find(|(s, _)| s.trim() == text)
            .unwrap_or_else(|| panic!("на экране нет «{text}»"))
            .1
            .center()
    }

    fn click(ctx: &egui::Context, a: &mut App, pos: egui::Pos2) {
        let down = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(ctx, a, vec![egui::Event::PointerMoved(pos), down(true)]);
        frame(ctx, a, vec![down(false)]);
    }

    /// Заполненный приём: ребёнок загружен, осмотр вписан.
    fn at_work(a: &mut App) {
        a.open_odt(&sample());
        assert!(a.nap.is_some());
        a.pat.complaints = "кашель".into();
        a.pat.chdd = "19".into();
        a.pat.chss = "82".into();
        a.pat.diagnosis = "J06.9 Острая инфекция верхних дыхательных путей".into();
        a.pat.anamnesis_disease = "болеет три дня".into();
        a.set.doc_mut().name = "Примерова П. П.".into();
    }

    /// Жалоба врача: «заполнила форму, добавила врача — всё слетело».
    ///
    /// Поля пациента при этом целы (проверяется соседним тестом): слетает то,
    /// что видно в окне настроек. `add_doctor` делает активным ЧИСТЫЙ профиль,
    /// и фамилия, должность, заведующий, печать и подпись разом пустеют.
    /// Молча этого делать нельзя — окно обязано сказать, что случилось и что
    /// прежний врач никуда не делся.
    #[test]
    fn adding_a_doctor_says_the_blank_form_is_a_new_profile() {
        let ctx = egui::Context::default();
        let mut a = bare();
        at_work(&mut a);
        a.set.doc_mut().post = "врач-педиатр участковый".into();

        let items = frame(&ctx, &mut a, vec![]);
        click(&ctx, &mut a, at(&items, "Настройки"));
        let items = frame(&ctx, &mut a, vec![]);
        click(&ctx, &mut a, at(&items, "Добавить врача"));
        let items = frame(&ctx, &mut a, vec![]);

        assert_eq!(a.set.doctors.len(), 2, "новый профиль не завёлся");
        assert!(a.set.doc().is_blank(), "новый профиль должен быть чистым");
        assert!(
            items
                .iter()
                .any(|(s, _)| s.contains("Прежний врач не потерян")),
            "пустые поля ничем не объяснены: {:?}",
            items.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(
            a.set.doctors[0].name, "Примерова П. П.",
            "прежний врач стёрт"
        );
        assert_eq!(a.set.doctors[0].post, "врач-педиатр участковый");
    }

    /// Правило проекта: ничто в настройках не имеет права стереть то, что
    /// врач набрала руками на пациента. Нажимаем подряд всё, что нарисовано
    /// Авторство и условия должны быть достижимы из самой программы, а не
    /// только лежать в исходниках. Щелчок по строке автора внизу открывает
    /// окно — если вёрстку перетасуют и строка перестанет быть кликабельной,
    /// это молча оставит программу без единственного места, где сказано,
    /// кто автор и на каких условиях она здесь работает.
    #[test]
    fn the_author_line_opens_the_terms() {
        let ctx = egui::Context::default();
        let mut a = bare();
        let items = frame(&ctx, &mut a, vec![]);
        let line = format!("{} · {}", crate::АВТОР, env!("CARGO_PKG_VERSION"));
        let pos = at(&items, &line);
        click(&ctx, &mut a, pos);
        let items = frame(&ctx, &mut a, vec![]);
        assert!(a.show_about, "щелчок по строке автора не открыл окно");
        // Заголовки блоков `ui::section` печатает прописными, поэтому
        // сравниваем без учёта регистра.
        let text: String = items
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<String>()
            .to_lowercase();
        assert!(text.contains("условия использования"), "условий в окне нет");
        assert!(
            text.contains("врачебных решений"),
            "в условиях нет главного — что врачебные решения принимает врач"
        );
        assert!(
            text.contains(&crate::АВТОР.to_lowercase()),
            "автор не назван"
        );
    }

    /// Её слова: «Че то у меня не собирается пакет… Он мне не собирает,
    /// только направление даёт». Пакет при этом собирался исправно — не было
    /// выписки, потому что врач, правя текст осмотра, стёрла подстановки, и
    /// сохранение оказалось заблокировано. Со вкладки «Пакет» причины не было
    /// видно вовсе, и выглядело это как сломанная сборка пакета.
    #[test]
    fn the_package_tab_says_why_the_vypiska_is_missing() {
        let ctx = egui::Context::default();
        let mut a = bare();
        a.open_odt(&sample());
        a.tab = Tab::Package;
        a.sources.push(package::Source {
            name: "referral.odt".into(),
            bytes: b"x".to_vec(),
            kind: package::Kind::Referral,
        });
        // Ровно её случай: текст осмотра без подстановок.
        a.set.exam_template = "Состояние удовлетворительное.".into();

        let items = frame(&ctx, &mut a, vec![]);
        let all: String = items.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            all.contains("Выписки в пакете нет"),
            "на вкладке «Пакет» не сказано, что выписки в нём нет: {all}"
        );
        assert!(
            all.contains("Текст осмотра"),
            "не названо место, где это чинится: {all}"
        );
    }

    /// Верхний ряд должен читаться как один ряд: выбор врача и «Настройки»
    /// одной высоты и стоят вровень.
    ///
    /// Сами по себе они разной высоты — выбор врача набран крупнее (14 pt
    /// полужирным против 12.5 у кнопки), и низ у соседей разъезжается на
    /// пару пикселей. Глазом это видно сразу, компилятору — никогда.
    #[test]
    fn the_top_row_stands_on_one_line() {
        let ctx = egui::Context::default();
        let mut a = bare();
        let title = a.set.doc().title().to_string();
        let sh = paint(&ctx, &mut a, vec![]);
        let doc = box_of(&sh, &title).expect("нет рамки у выбора врача");
        let set = box_of(&sh, "Настройки").expect("нет рамки у кнопки настроек");
        assert!(
            (doc.top() - set.top()).abs() < 0.5 && (doc.bottom() - set.bottom()).abs() < 0.5,
            "верхний ряд разъехался: врач {doc:?}, «Настройки» {set:?}"
        );
    }

    /// Кликабельное должно быть видно кликабельным ДО щелчка. Строка автора
    /// внизу выглядит как обычная подпись, и без отклика на наведение никто
    /// не догадается, что за ней окно с условиями: подсказку ещё надо
    /// дождаться, а до неё строка мертва.
    #[test]
    fn the_author_line_lights_up_under_the_cursor() {
        let ctx = egui::Context::default();
        let mut a = bare();
        let line = format!("{} · {}", crate::АВТОР, env!("CARGO_PKG_VERSION"));
        let pos = at(&frame(&ctx, &mut a, vec![]), &line);
        let cold = ink_of(&paint(&ctx, &mut a, vec![]), &line);
        let hot = ink_of(
            &paint(&ctx, &mut a, vec![egui::Event::PointerMoved(pos)]),
            &line,
        );
        assert!(
            cold.is_some() && hot.is_some(),
            "строки автора нет на экране"
        );
        assert_ne!(cold, hot, "строка автора не отзывается на наведение");
    }

    /// Нижний ряд читается слева направо: сначала главное действие, потом
    /// пояснение к нему, и только у самого края — авторство.
    ///
    /// Проверка не косметическая. Раскладка там задана справа налево, чтобы
    /// прижать авторство к краю, и вложенный `horizontal_wrapped` наследовал
    /// это направление: ряд молча переворачивался — кнопка уезжала вправо,
    /// сообщение вставало слева от неё. Компилятор такое не видит, тесты на
    /// нажатия тоже: они ищут надпись по тексту, а не по месту.
    #[test]
    fn the_button_leads_the_bottom_row_and_the_author_closes_it() {
        let ctx = egui::Context::default();
        let mut a = bare();
        let items = frame(&ctx, &mut a, vec![]);
        let x = |t: &str| at(&items, t).x;
        let btn = x("Сохранить выписку");
        let hint = x("Сначала загрузите направление из КУМки");
        let author = x(&format!("{} · {}", crate::АВТОР, env!("CARGO_PKG_VERSION")));
        assert!(
            btn < hint,
            "кнопка должна идти первой, а не после пояснения"
        );
        assert!(hint < author, "авторство должно стоять последним, у края");
    }

    /// Правило проекта: ничто в настройках не имеет права стереть то, что
    /// врач набрала руками на пациента. Нажимаем подряд всё, что нарисовано
    /// в окне настроек, и сверяем поля пациента с исходными.
    #[test]
    fn nothing_in_the_settings_window_wipes_the_patient() {
        // Эти поднимают системный диалог — в тесте он заблокировал бы поток.
        let dialogs = ["Выбрать…", "Сохранить выписку", "Выбрать файл…"];

        // Цели — только то, что появляется ПРИ ОТКРЫТЫХ настройках. Основное
        // окно никуда не девается под ними, и его кнопки («Подставить
        // целиком», «Другой пациент…») поля пациента меняют — это их работа,
        // тест не про них. Перечислять их по одной значит чинить тест после
        // каждой правки интерфейса, поэтому берём разницу двух кадров.
        let labels = |show_settings: bool| -> Vec<String> {
            let ctx = egui::Context::default();
            let mut probe = bare();
            at_work(&mut probe);
            probe.set_backup = Some(probe.set.clone());
            probe.show_settings = show_settings;
            frame(&ctx, &mut probe, vec![]);
            frame(&ctx, &mut probe, vec![])
                .iter()
                .map(|(s, _)| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let body = labels(false);
        let targets: Vec<String> = labels(true)
            .into_iter()
            .filter(|s| !dialogs.contains(&s.as_str()) && !body.contains(s))
            .collect();
        // Порог грубый намеренно: это проверка «окно вообще отрисовалось», а
        // не счёт надписей. Точное число ломалось бы от любой правки вёрстки.
        assert!(
            targets.len() >= 15,
            "окно настроек не отрисовалось: {targets:?}"
        );

        for target in targets {
            let ctx = egui::Context::default();
            let mut a = bare();
            at_work(&mut a);
            let before = a.pat.clone();
            a.set_backup = Some(a.set.clone());
            a.show_settings = true;
            frame(&ctx, &mut a, vec![]);
            let items = frame(&ctx, &mut a, vec![]);
            let Some((_, r)) = items.iter().find(|(s, _)| s.trim() == target) else {
                continue;
            };
            let pos = r.center();
            click(&ctx, &mut a, pos);
            frame(&ctx, &mut a, vec![]);
            assert!(
                a.pat == before,
                "нажатие «{target}» изменило поля пациента: жалобы {:?}, диагноз {:?}",
                a.pat.complaints,
                a.pat.diagnosis
            );
        }
    }

    /// «И с печатью не поняла. Её нет.» Печать не загружена — документ
    /// выйдет без неё, и сказать об этом надо до сохранения, а не показать
    /// пустое место в готовом PDF.
    #[test]
    fn missing_stamp_is_named_before_saving() {
        let ctx = egui::Context::default();
        let mut a = bare();
        at_work(&mut a);
        assert!(
            a.problems().is_empty(),
            "документ должен быть готов к сохранению"
        );

        let items = frame(&ctx, &mut a, vec![]);
        assert!(
            items
                .iter()
                .any(|(s, _)| s.contains("не загружены — документ выйдет без них")),
            "про отсутствующую печать не сказано: {:?}",
            items.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>()
        );

        // Печать снята галочкой — тогда её отсутствие осознанно, и молчим.
        a.set.overlay = false;
        let items = frame(&ctx, &mut a, vec![]);
        assert!(!items.iter().any(|(s, _)| s.contains("документ выйдет без")));

        // Файл указан — напоминать не о чем.
        a.set.overlay = true;
        a.set.doc_mut().stamp_path = "D:/печать.jpg".into();
        a.set.doc_mut().sign_path = "D:/sign-sample.png".into();
        let items = frame(&ctx, &mut a, vec![]);
        assert!(!items.iter().any(|(s, _)| s.contains("не загружен")));
    }
}
