//! Карточки детей: то, что имеет смысл помнить между визитами.
//!
//! ПЕРСОНАЛЬНЫЕ МЕДИЦИНСКИЕ ДАННЫЕ. Лежат только на этой машине — в подпапке
//! `пациенты` рядом с настройками, то есть рядом с программой (она
//! переносимая). Если писать туда нельзя — запасным путём в `%APPDATA%` на
//! Windows и в `~/.config/vypiska` на Linux (см. `settings::dir`). Никуда не
//! передаются: сетевого кода в модуле нет и быть не должно. Карточки уезжают
//! вместе с флешкой, которую врач носит сам.
//!
//! Зачем: у хроника — эпилепсия, астма, нефрология — анамнез копится годами, и
//! на повторном направлении его набирают заново. Подгрузили прошлое, дописали
//! новое.
//!
//! ЛЕЖАТ ЗАШИФРОВАННЫМИ НА ОБЕИХ СИСТЕМАХ. Потерянная флешка иначе означает
//! ФИО, диагнозы и анамнезы детей открытым текстом. Пароль тут не годится:
//! вводить его на каждый запуск врач не станет, через неделю он окажется на
//! бумажке под клавиатурой. Защита обязана работать молча.
//!
//! На Windows это DPAPI: ключ хранит система и привязывает к учётной записи.
//!
//! На Linux системного эквивалента нет, поэтому ключ свой: 32 случайных байта
//! в `~/.config/vypiska-ключ` с правами 0600, а карточки — XChaCha20-Poly1305
//! (шифр с проверкой целостности: подменённый байт обнаруживается, а не
//! становится мусором в анамнезе). Главное здесь — ключ лежит НЕ рядом с
//! карточками: карточки едут на флешке вместе с программой, ключ остаётся в
//! домашней папке врача. Унесли флешку — унесли только шифротекст.
//!
//! ОТ ЧЕГО ЭТО НЕ ЗАЩИЩАЕТ, честно (Linux):
//!
//! * от того, кто получил доступ к учётной записи врача (живой сеанс,
//!   подобранный пароль, root машины, администратор домена): ключ читается
//!   теми же правами, что и всё остальное в домашней папке;
//! * от копии ВСЕЙ домашней папки вместе с `~/.config/vypiska-ключ` —
//!   например, из бэкапа сетевого домашнего каталога;
//! * от того, кто смотрит на экран или в память запущенной программы.
//!
//! Защищает ровно от реального сценария: флешку/папку с карточками потеряли,
//! скопировали, воткнули в чужой компьютер. Там их нечем открыть.
//!
//! Секрет-бэкенд рабочего стола (Secret Service/GNOME Keyring) сюда не взят
//! сознательно: он тянет D-Bus, требует разблокированной связки и на «Альте» с
//! MATE в домене даёт новый способ НЕ открыть карточку в разгар приёма. Файл
//! 0600 закрывает тот же сценарий без единой лишней зависимости.

// `anyhow!` нужен только unix-ветке: на Windows он остался бы неиспользованным
// импортом и предупреждением в сборке врача.
#[cfg(unix)]
use anyhow::anyhow;
use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::ffi::c_void;
use std::path::{Path, PathBuf};

/// Дата в том же виде, что в выписке: «17.03.2026».
const DATE: &str = "%d.%m.%Y";

/// Метки контейнеров. По ним отличаем зашифрованную карточку от старой,
/// которая лежит открытым JSON: без метки «не расшифровалось» пришлось бы
/// толковать наугад — то ли чужая учётная запись, то ли файл ещё не шифровали,
/// а это разные фразы для врача. Метки разные у систем, потому что и
/// контейнеры разные: DPAPI-блоб на Linux нечем открыть, наш — на Windows.
const MAGIC_WIN: &[u8] = b"vypiska-dpapi1\n";
const MAGIC_NIX: &[u8] = b"vypiska-xchacha1\n";

/// Свой контейнер — тот, который эта система умеет писать и читать.
#[cfg(windows)]
const MAGIC: &[u8] = MAGIC_WIN;
#[cfg(unix)]
const MAGIC: &[u8] = MAGIC_NIX;

/// Чужой — принесённый с машины на другой системе.
#[cfg(windows)]
const FOREIGN: &[u8] = MAGIC_NIX;
#[cfg(unix)]
const FOREIGN: &[u8] = MAGIC_WIN;

/// Отдельная фраза: «принесли с другой системы» — не то же самое, что «ключ не
/// тот», и заводить карточку заново поверх тут точно не надо.
const FOREIGN_TEXT: &str =
    "карточку заводили на компьютере с другой операционной системой — открыть её можно только там";

/// Никаких окон от Windows: DPAPI зовётся прямо из кадра отрисовки, и
/// модальный диалог посреди него выглядит как зависшая программа.
#[cfg(windows)]
const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

/// Ровно то, что ждёт crypt32: длина и указатель.
#[cfg(windows)]
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

/// Обе функции crypt32 различаются только вторым параметром (на запись — текст
/// описания, на чтение — место под него), а мы в обоих случаях передаём NULL:
/// описание пришлось бы хранить открытым. Поэтому подпись общая и вызов один
/// на двоих.
#[cfg(windows)]
type Dpapi = unsafe extern "system" fn(
    *const DataBlob,
    *mut c_void,
    *const DataBlob,
    *mut c_void,
    *mut c_void,
    u32,
    *mut DataBlob,
) -> i32;

// Объявлено руками: ради двух функций тянуть в дерево зависимостей ещё один
// крейт незачем. DPAPI есть с Windows 2000 — на целевой Windows 7 тоже.
// Только Windows: crypt32/kernel32 на Linux не существует, и линковщик там
// не нашёл бы этих имён — отсюда cfg на весь блок, а не просто на вызовы.
#[cfg(windows)]
#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        descr: *mut c_void,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
    fn CryptUnprotectData(
        data_in: *const DataBlob,
        descr: *mut c_void,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

/// Один вызов DPAPI на оба направления.
///
/// `None` — Windows отказал. На записи это «не зашифровалось», на чтении —
/// «эти данные не для этой учётной записи»: чаще всего флешку принесли на
/// чужой компьютер. Причину наверх поднимает вызывающий, здесь её нет.
#[cfg(windows)]
fn dpapi(call: Dpapi, src: &[u8]) -> Option<Vec<u8>> {
    let input = DataBlob {
        cb_data: u32::try_from(src.len()).ok()?,
        pb_data: src.as_ptr() as *mut u8,
    };
    let mut out = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    // SAFETY: обе структуры живут до конца вызова; crypt32 читает вход по длине
    // cb_data и не пишет в него, а результат отдаёт своей памятью, которую мы
    // копируем и тут же возвращаем системе.
    unsafe {
        let ok = call(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 || out.pb_data.is_null() {
            return None;
        }
        let copy = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec();
        // Память выделена LocalAlloc внутри crypt32: без LocalFree подтекало бы
        // каждое сохранение и каждый показ списка последних пациентов.
        LocalFree(out.pb_data as *mut c_void);
        Some(copy)
    }
}

/// Расшифровать то, что после метки `MAGIC`. `Err` — карточка есть, но её
/// нечем открыть; текст написан для врача и уходит наверх как есть.
#[cfg(windows)]
fn unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
    dpapi(CryptUnprotectData, cipher).context(
        "карточка зашифрована другой учётной записью Windows — её видно только на том компьютере, где заводили",
    )
}

// ── Linux: свой ключ + XChaCha20-Poly1305 ───────────────────────────────────
//
// Почему не «что уже лежит в дереве» (aes, cbc, chacha20 без аутентификации):
// голый поток шифра без метки подлинности не отличает расшифровку от мусора —
// подменённый байт молча превратится в другой текст внутри анамнеза, а неверный
// ключ выдаст не ошибку, а случайные байты. Складывать шифр с MAC руками —
// именно тот код, который потом ломается в мелочи. Поэтому взят готовый AEAD.
//
// XChaCha20 (а не ChaCha20) ради 24-байтного одноразового номера: его можно
// брать просто случайным на каждое сохранение, без счётчика, который негде
// хранить на переносимой флешке.

/// Длина одноразового номера XChaCha20 — он лежит в файле сразу после метки.
#[cfg(unix)]
const NONCE: usize = 24;

/// Случайные байты. `/dev/urandom` есть на любом Linux с 1994 года; отдельный
/// крейт ради `open`+`read_exact` был бы лишним.
#[cfg(unix)]
fn random(buf: &mut [u8]) -> Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .context("нет источника случайных чисел /dev/urandom — карточка не сохранена")
}

/// Где лежит ключ. НЕ в папке с карточками и не в папке программы: они уезжают
/// на флешке, и ключ уехал бы вместе с ними. `~/.config/vypiska-ключ` —
/// соседний файл, а не содержимое `~/.config/vypiska`, чтобы копия папки
/// настроек тоже не прихватила его заодно.
///
/// Если домашней папки нет ($HOME не задан) — это ошибка, а не повод положить
/// ключ во временный каталог: после перезагрузки там пусто, и карточки
/// перестали бы открываться навсегда.
#[cfg(unix)]
fn key_file() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(p) = tests::key_override() {
        return Ok(p);
    }
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("не найдена домашняя папка ($HOME) — ключу от карточек негде лежать")?;
    Ok(home.join("vypiska-ключ"))
}

/// `Ok(None)` — ключа ещё нет (первый запуск на этой машине). Файл не той
/// длины — громкая ошибка: молча завести новый ключ значит навсегда закрыть
/// все карточки, которые этим ключом уже зашифрованы.
#[cfg(unix)]
fn read_secret() -> Result<Option<[u8; 32]>> {
    let p = key_file()?;
    match std::fs::read(&p) {
        Ok(b) => Ok(Some(b.try_into().map_err(|_| {
            anyhow!(
                "файл ключа {} повреждён: в нём не 32 байта. Не заменяйте его новым — тогда ни одна карточка больше не откроется; восстановите файл из резервной копии",
                p.display()
            )
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context(format!("не прочитать файл ключа {}", p.display())),
    }
}

/// Ключ для записи: читаем, а если его ещё нет — заводим.
#[cfg(unix)]
fn secret_or_new() -> Result<[u8; 32]> {
    if let Some(k) = read_secret()? {
        return Ok(k);
    }
    let p = key_file()?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).context("не создать папку для ключа")?;
    }
    let mut fresh = [0u8; 32];
    random(&mut fresh)?;

    // Через временный файл и hard link, а не прямую запись: оборванная запись
    // оставила бы обрезанный ключ, а `link` вдобавок не затрёт ключ, который в
    // ту же секунду завёл второй запущенный экземпляр программы — иначе его
    // только что сохранённая карточка перестала бы открываться.
    let tmp = p.with_extension("новый");
    {
        use std::io::Write;
        create_0600(&tmp)?
            .write_all(&fresh)
            .context("не записать ключ")?;
    }
    let linked = std::fs::hard_link(&tmp, &p);
    let _ = std::fs::remove_file(&tmp);
    if let Err(e) = linked {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(e).context(format!("не положить ключ в {}", p.display()));
        }
    }
    // Читаем с диска, а не возвращаем `fresh`: если ключ успел завести кто-то
    // другой, шифровать надо тем, что лежит на месте.
    read_secret()?.context("ключ исчез сразу после создания")
}

/// Зашифровать карточку: метка, одноразовый номер, дальше шифротекст с меткой
/// подлинности. Ключ берётся (или заводится) снаружи папки с карточками.
#[cfg(unix)]
fn seal(json: &str) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};

    let key = secret_or_new()?;
    let mut nonce = [0u8; NONCE];
    random(&mut nonce)?;
    let body = XChaCha20Poly1305::new(&Key::from(key))
        .encrypt(&XNonce::from(nonce), json.as_bytes())
        .map_err(|_| anyhow!("не зашифровать карточку, она не сохранена"))?;

    let mut blob = MAGIC.to_vec();
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&body);
    Ok(blob)
}

/// Обратная сторона `seal`. Любой отказ — `Err` со словами для врача:
/// «не открылось» и «ребёнка не было» обязаны звучать по-разному.
#[cfg(unix)]
fn unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};

    let key = read_secret()?.with_context(|| {
        format!(
            "карточка зашифрована, а ключа на этом компьютере нет ({}): её заводили на другой машине или под другой учётной записью. Данные целы, но открыть их здесь нечем",
            key_file().map(|p| p.display().to_string()).unwrap_or_default()
        )
    })?;
    if cipher.len() < NONCE {
        bail!("карточка повреждена: файл обрезан");
    }
    let (nonce, body) = cipher.split_at(NONCE);
    let mut n = [0u8; NONCE];
    n.copy_from_slice(nonce);
    XChaCha20Poly1305::new(&Key::from(key))
        .decrypt(&XNonce::from(n), body)
        .map_err(|_| {
            anyhow!("карточку не открыть здешним ключом: её заводили на другом компьютере или под другой учётной записью, либо файл подменили. Заводить анамнез заново поверх неё нельзя — сначала найдите тот компьютер")
        })
}

/// Файл, который с первого мига существует с правами 0600.
///
/// Права выставляются В МОМЕНТ создания, а не после записи: запись с
/// последующим chmod оставляла бы щель, в которой файл лежит с обычными
/// правами (по умолчанию 0644, читают все). На машине в домене, где на
/// компьютер заходят и другие учётные записи, это ровно та щель, ради закрытия
/// которой права и ставятся. Ключу это нужно не меньше, чем карточке.
///
/// `create_new` заодно не даст записать поверх чужого файла по подсунутой
/// ссылке: временное имя предсказуемо, а папка на сетевом домашнем каталоге
/// может быть доступна не только ей.
#[cfg(unix)]
fn create_0600(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let open = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    };
    open()
        .or_else(|e| {
            // Остаток от прерванного сохранения — свой же файл, его можно
            // убрать и создать заново, но только если он действительно наш.
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                std::fs::remove_file(path)?;
                open()
            } else {
                Err(e)
            }
        })
        .with_context(|| format!("не создать файл {}", path.display()))
}

/// Карточка ребёнка: то, что имеет смысл помнить между визитами.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Card {
    pub snils: String,
    pub fio: String,
    pub phone: String,
    pub birth_cert: String,
    /// В поле документа — паспорт, а не свидетельство о рождении. Старые
    /// карточки этого ключа не знают и читаются как свидетельство — так и
    /// было до появления выбора.
    pub passport: bool,
    pub organized: String,
    pub diagnosis: String,
    pub anamnesis_life: String,
    pub anamnesis_disease: String,
    /// Дата последнего сохранения, «17.03.2026» — показываем врачу, откуда текст.
    pub updated: String,
}

/// Строка для списка последних пациентов.
#[derive(Debug, Clone)]
pub struct Entry {
    pub snils: String,
    pub fio: String,
    pub updated: String,
    /// Карточка есть, но не расшифровалась. ФИО и дата пустые не потому, что их
    /// не заполнили, а потому что файл закрыт чужим ключом: другой учётной
    /// записью, другой машиной или другой операционной системой.
    /// Интерфейс обязан написать это словами: пустая строка в списке читается
    /// как «карточка пустая», а это неправда.
    pub locked: bool,
}

fn dir() -> PathBuf {
    crate::settings::dir().join("пациенты")
}

/// Имя файла — одни цифры СНИЛС. Иначе «112-233-445 95» и «11223344595» завели
/// бы две карточки на одного ребёнка, и половина анамнеза потерялась бы во
/// второй. Побочно это отрезает любой путь из ввода: ни точки, ни слэша,
/// ни двоеточия фильтр не пропускает — из пользовательской строки в путь
/// уходит только `[0-9]{11}`.
///
/// ФИО в имени файла нет и быть не должно — оно уехало бы с флешкой мимо всего
/// шифрования. СНИЛС оставлен сознательно: сам по себе он не говорит ни имени,
/// ни диагноза, зато делает имя файла вычислимым — на этом держится и «один
/// ребёнок = одна карточка», и атомарная замена через rename. Спрятать его
/// хешем не выйдет: номеров всего 10^9 с контрольным числом, перебор считается
/// на ноутбуке за минуты, а `DefaultHasher` вдобавок не обещает одинаковый
/// результат между версиями Rust — после обновления тулчейна врач не нашёл бы
/// ни одной карточки.
fn key(snils: &str) -> Result<String> {
    let d: String = snils
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(11)
        .collect();
    if d.len() < 11 {
        bail!("СНИЛС неполный: нужно 11 цифр, набрано {}", d.len());
    }
    Ok(d)
}

fn file(dir: &Path, snils: &str) -> Result<PathBuf> {
    Ok(dir.join(format!("{}.json", key(snils)?)))
}

/// Дату ставим мы, а не врач: «последнее сохранение» — это момент записи.
fn save_in(dir: &Path, card: &Card, today: &str) -> Result<()> {
    let path = file(dir, &card.snils)?;
    let tmp = path.with_extension("json.tmp");
    let card = Card {
        updated: today.to_string(),
        ..card.clone()
    };
    std::fs::create_dir_all(dir).context("не создать папку пациентов")?;
    // Шифруем (на Windows) до записи, а не после: открытый JSON не должен
    // оказаться на диске даже на миг — временный файл переживёт выдернутую
    // флешку ровно так же.
    write_protected(&tmp, &serde_json::to_string_pretty(&card)?)?;
    // Через временный файл и rename: обрыв записи (вынули флешку) не должен
    // оставить обрезанный файл — карточка молча стала бы нечитаемой.
    std::fs::rename(&tmp, &path).context("не заменить карточку")?;
    Ok(())
}

/// Записать карточку в защищённом виде: на Windows — контейнер DPAPI, на Linux
/// — контейнер XChaCha20-Poly1305 на ключе из домашней папки. На Linux к
/// шифрованию добавлены права 0600 — они не заменяют его, а закрывают вторую
/// дверь: других пользователей ЭТОЙ машины.
#[cfg(windows)]
fn write_protected(tmp: &Path, json: &str) -> Result<()> {
    let mut blob = MAGIC.to_vec();
    blob.extend_from_slice(
        &dpapi(CryptProtectData, json.as_bytes())
            .context("Windows отказался зашифровать карточку, она не сохранена")?,
    );
    std::fs::write(tmp, &blob).context("не записать карточку")
}

#[cfg(unix)]
fn write_protected(tmp: &Path, json: &str) -> Result<()> {
    use std::io::Write;

    // Шифруем ДО создания файла: открытый JSON не должен оказаться на диске
    // даже на миг, и неудача шифрования не должна оставить пустой файл.
    let blob = seal(json)?;
    create_0600(tmp)?
        .write_all(&blob)
        .context("не записать карточку")
}

/// `Ok(None)` — карточки нет, ребёнок новый. `Err` — карточка есть, но не
/// читается: файл побит, или зашифрован другой учётной записью (флешку
/// принесли на чужой компьютер), или пришёл с машины на другой системе.
///
/// Разница обязана дойти до интерфейса. Отдать в обоих случаях «ничего» значит
/// сказать врачу, что ребёнок новый: он наберёт анамнез с нуля и сохранит его
/// поверх — накопленные годами записи заменит пустота. Тихий отказ здесь
/// опаснее громкого.
fn load_in(dir: &Path, snils: &str) -> Result<Option<Card>> {
    let raw = match std::fs::read(file(dir, snils)?) {
        Ok(raw) => raw,
        // Ни файла, ни папки — этого ребёнка просто не было.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("карточка на диске есть, но не читается"),
    };
    let json = if raw.starts_with(MAGIC) {
        unprotect(&raw[MAGIC.len()..])?
    } else if raw.starts_with(FOREIGN) {
        bail!("{FOREIGN_TEXT}");
    } else {
        // Карточка, заведённая до шифрования. Читаем как есть: терять из-за
        // перехода накопленный анамнез нельзя. Ближайшее сохранение положит её
        // уже зашифрованной.
        raw
    };
    let mut card: Card = serde_json::from_slice(&json).context("карточка повреждена")?;
    // Файл могли поправить руками, а карточка без СНИЛС потом не сохранится —
    // восстанавливаем из имени файла.
    if card.snils.trim().is_empty() {
        card.snils = crate::mask::snils(snils);
    }
    Ok(Some(card))
}

fn recent_in(dir: &Path, limit: usize) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // ponytail: расшифровываем все карточки на каждый вызов. Отдельный индекс
    // ФИО завести нельзя — открытый список имён сводит шифрование на нет, а
    // карточек у участкового единицы.
    let mut out: Vec<Entry> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            // Не карточка, а посторонний файл в папке: мимо, без строки в списке.
            key(&name).ok()?;
            match load_in(dir, &name) {
                Ok(card) => card.map(|card| Entry {
                    snils: card.snils,
                    fio: card.fio,
                    updated: card.updated,
                    locked: false,
                }),
                // Выбросить нечитаемую карточку из списка — соврать, что этого
                // ребёнка не было. ФИО и дату честно не знаем: файл не открылся.
                Err(_) => Some(Entry {
                    snils: crate::mask::snils(&name),
                    fio: String::new(),
                    updated: String::new(),
                    locked: true,
                }),
            }
        })
        .collect();
    // Новые сверху; нечитаемые уходят вниз сами — даты у них нет. ФИО — только
    // чтобы при равных датах список не прыгал от запуска к запуску: порядок
    // файлов в папке произвольный.
    out.sort_by(|a, b| {
        day(&b.updated)
            .cmp(&day(&a.updated))
            .then_with(|| a.fio.cmp(&b.fio))
    });
    out.truncate(limit);
    out
}

/// Нечитаемая дата уходит в конец списка, а не роняет сортировку.
fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, DATE).unwrap_or(NaiveDate::MIN)
}

fn forget_in(dir: &Path, snils: &str) -> Result<()> {
    match std::fs::remove_file(file(dir, snils)?) {
        // Карточки и так нет — врач хотел ровно этого.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r.context("не удалить карточку"),
    }
}

pub fn save(card: &Card) -> Result<()> {
    save_in(&dir(), card, &crate::fmt::today().format(DATE).to_string())
}

/// `Ok(None)` — такого ребёнка не было. `Err` — карточка есть, но закрыта или
/// побита; текст ошибки написан для врача, его можно показать как есть.
pub fn load(snils: &str) -> Result<Option<Card>> {
    load_in(&dir(), snils)
}

pub fn recent(limit: usize) -> Vec<Entry> {
    recent_in(&dir(), limit)
}

pub fn forget(snils: &str) -> Result<()> {
    forget_in(&dir(), snils)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тесты не пишут в папку программы: карточки пациентов — не то, что можно
    /// случайно насыпать на машину разработчика. Каталог чистится на входе,
    /// поэтому после упавшего теста файлы остаются для разбора.
    fn dir_for_tests(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vypiska-тест-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        #[cfg(unix)]
        KEY.with(|k| {
            let p = std::env::temp_dir()
                .join(format!("vypiska-тест-ключ-{}-{name}", std::process::id()));
            let _ = std::fs::remove_file(&p);
            *k.borrow_mut() = Some(p);
        });
        d
    }

    // Ключ от карточек в тестах: свой у каждого теста и всегда во временной
    // папке. Писать в настоящий `~/.config` тесты не должны, а один ключ на
    // всех ломался бы ровно там, где тест ключ подменяет, — соседние тесты
    // идут в тех же секундах. Каждый тест cargo запускает в своём потоке,
    // поэтому переменная потоковая, а не глобальная.
    #[cfg(unix)]
    thread_local! {
        static KEY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    #[cfg(unix)]
    pub(super) fn key_override() -> Option<PathBuf> {
        KEY.with(|k| k.borrow().clone())
    }

    /// Поля перечислены без `..Default::default()` намеренно: новое поле в
    /// `Card` сломает компиляцию здесь, и круг «сохранить → прочитать» не
    /// начнёт молча его терять.
    fn full() -> Card {
        Card {
            snils: "112-233-445 95".into(),
            fio: "Иванов Иван Иванович".into(),
            phone: "89000000000".into(),
            birth_cert: "III-АА 000001".into(),
            passport: false,
            organized: "Школа (пример), 6 класс".into(),
            diagnosis: "Z00.1 Профилактический осмотр".into(),
            anamnesis_life: "Тестовые сведения об анамнезе жизни.".into(),
            anamnesis_disease: "Тестовые сведения об анамнезе заболевания.".into(),
            updated: String::new(),
        }
    }

    /// Ищем открытый текст в байтах файла: `contains` по строке тут не подходит,
    /// зашифрованный файл — не UTF-8.
    fn contains(hay: &[u8], needle: &str) -> bool {
        hay.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    fn files(d: &Path) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(d) else {
            return Vec::new();
        };
        let mut v: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let d = dir_for_tests("круг");
        save_in(&d, &full(), "17.03.2026").unwrap();
        let back = load_in(&d, "112-233-445 95").unwrap().unwrap();
        assert_eq!(
            back,
            Card {
                updated: "17.03.2026".into(),
                ..full()
            },
            "поле потерялось на круге сохранить -> прочитать"
        );
        assert_eq!(files(&d), ["11223344595.json"], "имя файла — цифры СНИЛС");
    }

    /// Две записи одного номера — один ребёнок. Иначе анамнез раздваивается,
    /// и врач находит половину.
    #[test]
    fn dashes_and_digits_are_the_same_child() {
        let d = dir_for_tests("один-ребёнок");
        save_in(&d, &full(), "17.03.2026").unwrap();
        let mut second = full();
        second.snils = "11223344595".into();
        second.anamnesis_disease = "Повторный осмотр без особенностей.".into();
        save_in(&d, &second, "18.03.2026").unwrap();

        assert_eq!(files(&d), ["11223344595.json"], "завелась вторая карточка");
        for form in ["112-233-445 95", "11223344595", "112 233 445 95"] {
            let got = load_in(&d, form).unwrap().unwrap();
            assert_eq!(
                got.anamnesis_disease, "Повторный осмотр без особенностей.",
                "{form}"
            );
        }
        assert_eq!(recent_in(&d, 10).len(), 1);
    }

    #[test]
    fn empty_and_short_snils_are_refused() {
        let d = dir_for_tests("короткий-снилс");
        for bad in ["", " ", "112-233-445", "1122334459", "abc"] {
            let card = Card {
                snils: bad.into(),
                ..full()
            };
            let err = save_in(&d, &card, "17.03.2026").unwrap_err().to_string();
            assert!(err.contains("СНИЛС"), "невнятная ошибка на «{bad}»: {err}");
            assert!(load_in(&d, bad).is_err(), "«{bad}» прошло в чтение");
            assert!(forget_in(&d, bad).is_err(), "«{bad}» прошло в удаление");
        }
        assert!(files(&d).is_empty(), "от плохого СНИЛС остались файлы");
    }

    /// Битый или чужой файл не роняет программу, но и не притворяется
    /// отсутствием карточки: «ребёнка не было» и «карточка не открывается» —
    /// разные новости, и вторая должна дойти до врача словами.
    #[test]
    fn missing_is_none_but_unreadable_is_an_error() {
        let d = dir_for_tests("битый-файл");
        assert!(
            load_in(&d, "112-233-445 95").unwrap().is_none(),
            "папки ещё нет"
        );

        std::fs::create_dir_all(&d).unwrap();
        // Обрыв записи до rename выглядел бы примерно так.
        std::fs::write(d.join("11223344595.json"), "{\"fio\":\"Иванов Ми").unwrap();
        std::fs::write(d.join("22334455661.json"), "").unwrap();
        // А так — карточка, зашифрованная на другом компьютере: метка наша,
        // расшифровать нечем.
        let mut alien = MAGIC.to_vec();
        alien.extend_from_slice(&[7u8; 64]);
        std::fs::write(d.join("33445566725.json"), &alien).unwrap();

        for s in ["11223344595", "223-344-556 61", "334-455-667 25"] {
            assert!(load_in(&d, s).is_err(), "битый {s} прочитался как пустой");
        }
        let list = recent_in(&d, 10);
        assert_eq!(list.len(), 3, "нечитаемые карточки исчезли из списка");
        assert!(
            list.iter().all(|e| e.locked && e.fio.is_empty()),
            "нечитаемая карточка притворилась обычной"
        );
        let mut nums: Vec<String> = list.iter().map(|e| e.snils.clone()).collect();
        nums.sort();
        assert_eq!(
            nums,
            ["112-233-445 95", "223-344-556 61", "334-455-667 25"],
            "по строке нельзя понять, чья это карточка"
        );
    }

    /// Открытый JSON на диске — это и есть та утечка, ради которой всё
    /// затевалось: флешку теряют вместе с диагнозами детей. Только Windows:
    /// это проверка настоящего шифрования DPAPI, а на Linux его нет (см.
    /// `unprotect`) — там своя версия теста, ниже.
    #[test]
    #[cfg(windows)]
    fn saved_card_is_not_readable_from_the_file() {
        let d = dir_for_tests("шифрование");
        save_in(&d, &full(), "17.03.2026").unwrap();

        let raw = std::fs::read(d.join("11223344595.json")).unwrap();
        assert!(raw.starts_with(MAGIC), "файл сохранён не как контейнер");
        for open in [
            "Иванов",
            "Профилактический осмотр",
            "89000000000",
            "Тестовые сведения",
        ] {
            assert!(!contains(&raw, open), "«{open}» лежит на диске открытым");
        }
    }

    /// То же самое на Linux: содержимое зашифровано, права 0600 никуда не
    /// делись, а ключ лежит НЕ в папке с карточками — иначе унесли папку,
    /// унесли и ключ, и всё шифрование остаётся только на вид.
    #[test]
    #[cfg(unix)]
    fn saved_card_is_encrypted_and_owner_only_on_linux() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir_for_tests("шифрование-linux");
        save_in(&d, &full(), "17.03.2026").unwrap();

        let path = d.join("11223344595.json");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "файл карточки доступен не только владельцу"
        );

        let raw = std::fs::read(&path).unwrap();
        assert!(raw.starts_with(MAGIC), "файл сохранён не как контейнер");
        for open in [
            "Иванов",
            "Профилактический осмотр",
            "89000000000",
            "Тестовые сведения",
            "III-АА",
            "112-233-445",
        ] {
            assert!(!contains(&raw, open), "«{open}» лежит на диске открытым");
        }

        let key = key_file().unwrap();
        assert!(
            !key.starts_with(&d),
            "ключ лежит рядом с карточками: {}",
            key.display()
        );
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "ключ доступен не только владельцу");
    }

    /// Папку с карточками скопировали и открывают на другой машине: ключ там
    /// другой. Это `Err` со словами, а не «такого ребёнка не было».
    #[test]
    #[cfg(unix)]
    fn card_from_another_machine_is_an_error_not_an_empty_card() {
        let d = dir_for_tests("чужой-ключ");
        save_in(&d, &full(), "17.03.2026").unwrap();
        assert!(
            load_in(&d, "11223344595").unwrap().is_some(),
            "своя же карточка не открылась"
        );

        // «Другая машина» — та же папка, но другой ключ в домашней папке.
        let mut other = [0u8; 32];
        random(&mut other).unwrap();
        std::fs::write(key_file().unwrap(), other).unwrap();

        let err = load_in(&d, "11223344595").unwrap_err().to_string();
        assert!(err.contains("ключ"), "невнятная ошибка врачу: {err}");
        let list = recent_in(&d, 10);
        assert_eq!(list.len(), 1, "нечитаемая карточка исчезла из списка");
        assert!(
            list[0].locked && list[0].fio.is_empty(),
            "нечитаемая карточка притворилась обычной"
        );
    }

    /// Ключа на машине нет вовсе (карточку принесли с другого компьютера).
    /// Молча завести новый ключ и отдать «ничего» нельзя.
    #[test]
    #[cfg(unix)]
    fn card_without_the_key_is_an_error() {
        let d = dir_for_tests("ключ-потерян");
        save_in(&d, &full(), "17.03.2026").unwrap();
        std::fs::remove_file(key_file().unwrap()).unwrap();

        let err = load_in(&d, "11223344595").unwrap_err().to_string();
        assert!(err.contains("ключ"), "невнятная ошибка врачу: {err}");
    }

    /// Подменённый байт обязан быть замечен. Шифр без метки подлинности выдал
    /// бы вместо ошибки другой текст внутри анамнеза — этого нельзя.
    #[test]
    #[cfg(unix)]
    fn a_flipped_byte_is_detected() {
        let d = dir_for_tests("подмена-байта");
        save_in(&d, &full(), "17.03.2026").unwrap();

        let path = d.join("11223344595.json");
        let mut raw = std::fs::read(&path).unwrap();
        let byte = MAGIC.len() + NONCE + 5;
        raw[byte] ^= 1;
        std::fs::write(&path, &raw).unwrap();

        let err = load_in(&d, "11223344595").unwrap_err().to_string();
        assert!(!err.is_empty(), "подмена байта прошла незамеченной");
    }

    /// Карточку принесли с машины на другой системе: её контейнер здесь не
    /// открыть в принципе, и сказать об этом надо иначе, чем «ключ не тот» —
    /// искать надо тот компьютер, а не свою связку ключей.
    #[test]
    fn a_card_from_the_other_system_says_so() {
        let d = dir_for_tests("чужая-система");
        std::fs::create_dir_all(&d).unwrap();
        let mut alien = FOREIGN.to_vec();
        alien.extend_from_slice(&[7u8; 64]);
        std::fs::write(d.join("11223344595.json"), &alien).unwrap();

        let err = load_in(&d, "11223344595").unwrap_err().to_string();
        assert!(
            err.contains("другой операционной системой"),
            "невнятная ошибка врачу: {err}"
        );
    }

    /// У врача уже накоплены карточки обычным JSON. Их нельзя ни потерять при
    /// переходе, ни оставить открытыми навсегда. Только Windows — на Linux
    /// «переход» никуда не ведёт: карточка и после сохранения остаётся
    /// читаемым JSON, просто с правами 0600 (см. тест выше).
    #[test]
    #[cfg(windows)]
    fn old_plain_card_is_read_and_encrypted_on_next_save() {
        let d = dir_for_tests("старый-json");
        std::fs::create_dir_all(&d).unwrap();
        let old = Card {
            updated: "01.02.2025".into(),
            ..full()
        };
        std::fs::write(
            d.join("11223344595.json"),
            serde_json::to_string_pretty(&old).unwrap(),
        )
        .unwrap();

        let got = load_in(&d, "11223344595").unwrap().unwrap();
        assert_eq!(got, old, "старая карточка прочиталась с потерями");

        save_in(&d, &got, "17.03.2026").unwrap();
        let raw = std::fs::read(d.join("11223344595.json")).unwrap();
        assert!(
            raw.starts_with(MAGIC),
            "пересохранение оставило файл открытым"
        );
        assert!(!contains(&raw, "Иванов"), "ФИО осталось на диске открытым");
        assert_eq!(
            load_in(&d, "11223344595").unwrap().unwrap().anamnesis_life,
            old.anamnesis_life,
            "анамнез не пережил перехода на шифрование"
        );
    }

    /// То же самое на Linux: у врача уже лежат карточки открытым JSON, их
    /// нельзя ни потерять при переходе, ни оставить открытыми навсегда.
    #[test]
    #[cfg(unix)]
    fn old_plain_card_is_read_and_resaved_on_linux() {
        let d = dir_for_tests("старый-json-linux");
        std::fs::create_dir_all(&d).unwrap();
        let old = Card {
            updated: "01.02.2025".into(),
            ..full()
        };
        std::fs::write(
            d.join("11223344595.json"),
            serde_json::to_string_pretty(&old).unwrap(),
        )
        .unwrap();

        let got = load_in(&d, "11223344595").unwrap().unwrap();
        assert_eq!(got, old, "старая карточка прочиталась с потерями");

        save_in(&d, &got, "17.03.2026").unwrap();
        let raw = std::fs::read(d.join("11223344595.json")).unwrap();
        assert!(
            raw.starts_with(MAGIC),
            "пересохранение оставило файл открытым"
        );
        assert!(!contains(&raw, "Иванов"), "ФИО осталось на диске открытым");
        assert_eq!(
            load_in(&d, "11223344595").unwrap().unwrap().anamnesis_life,
            old.anamnesis_life,
            "анамнез не пережил перехода на шифрование"
        );
    }

    /// Частичный JSON — валидный: `serde(default)` дополнит пустыми строками.
    /// СНИЛС при этом берётся из имени файла, иначе карточку не пересохранить.
    #[test]
    fn partial_json_loads_and_keeps_identity() {
        let d = dir_for_tests("частичный-json");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("11223344595.json"), r#"{"fio":"Иванов М. В."}"#).unwrap();

        let got = load_in(&d, "11223344595").unwrap().unwrap();
        assert_eq!(got.fio, "Иванов М. В.");
        assert_eq!(
            got.snils, "112-233-445 95",
            "СНИЛС не восстановлен из имени"
        );
        assert!(got.anamnesis_life.is_empty());
    }

    #[test]
    fn recent_puts_newest_first_and_respects_limit() {
        let d = dir_for_tests("последние");
        for (snils, fio, when) in [
            ("112-233-445 95", "Иванов М. В.", "17.03.2026"),
            ("223-344-556 61", "Петров А. А.", "01.09.2026"),
            ("334-455-667 25", "Сидорова В. С.", "15.12.2025"),
        ] {
            let card = Card {
                snils: snils.into(),
                fio: fio.into(),
                ..full()
            };
            save_in(&d, &card, when).unwrap();
        }

        let all = recent_in(&d, 10);
        let fio: Vec<&str> = all.iter().map(|e| e.fio.as_str()).collect();
        assert_eq!(fio, ["Петров А. А.", "Иванов М. В.", "Сидорова В. С."]);
        assert_eq!(all[0].updated, "01.09.2026");
        assert_eq!(
            all[0].snils, "223-344-556 61",
            "СНИЛС для повторной загрузки"
        );

        assert_eq!(recent_in(&d, 2).len(), 2, "лимит не соблюдён");
        assert_eq!(recent_in(&d, 2)[0].fio, "Петров А. А.");
        assert!(recent_in(&d, 0).is_empty());
    }

    /// Даты — не строки: «01.09.2026» позже «17.03.2026», хотя сравнение
    /// текстом поставило бы его ниже.
    #[test]
    fn recent_sorts_by_date_not_by_text() {
        let d = dir_for_tests("порядок-дат");
        for (snils, when) in [
            ("112-233-445 95", "09.01.2026"),
            ("223-344-556 61", "10.12.2025"),
        ] {
            let card = Card {
                snils: snils.into(),
                ..full()
            };
            save_in(&d, &card, when).unwrap();
        }
        let got = recent_in(&d, 10);
        assert_eq!(got[0].updated, "09.01.2026");
        assert_eq!(got[1].updated, "10.12.2025");
    }

    #[test]
    fn forget_removes_the_card() {
        let d = dir_for_tests("забыть");
        save_in(&d, &full(), "17.03.2026").unwrap();
        assert!(load_in(&d, "11223344595").unwrap().is_some());

        forget_in(&d, "112-233-445 95").unwrap();
        assert!(
            load_in(&d, "11223344595").unwrap().is_none(),
            "карточка осталась"
        );
        assert!(files(&d).is_empty());
        // Повторное удаление — не ошибка: результат тот же, карточки нет.
        forget_in(&d, "11223344595").unwrap();
    }
}
