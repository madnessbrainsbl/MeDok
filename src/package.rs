//! Пакет для краевой больницы: направление + выписка + обследования одним PDF.
//!
//! Принимающая сторона тяжёлые файлы не открывает, поэтому пакет собирается так
//! же, как врач делал его руками через Chrome и iLovePDF: каждая страница — JPEG
//! на листе A4, лишний вес снимается качеством сжатия, а не разрешением.
//!
//! Источники приходят вперемешку — фотографии с телефона, выгрузка сканера
//! айфона, чужие «compressed» PDF на шесть страниц, — и все проходят одним путём:
//! страница -> снимок -> A4 -> JPEG.

use anyhow::{bail, Context, Result};
use hayro::hayro_interpret::font::{FontData, FontQuery, StandardFont};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{page::Page, Pdf};
use hayro::{RenderCache, RenderSettings};
use image::RgbImage;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Ниже этой ширины пакет теряет юридический смысл: на 1240 px блок ЭЦП в
/// направлении (номер сертификата мелким шрифтом) ещё читается, а на меньшей
/// ширине рассыпается. Поэтому вес подгоняется только качеством JPEG.
pub const MIN_WIDTH: u32 = 1240;

/// Дно качества JPEG. Ниже печати идут квадратами, а подписи «плывут».
const MIN_JPEG: u8 = 35;

/// Меньше этого по любой стороне — это не страница, а вложенная картинка
/// (печать, подпись, логотип). Настоящий скан листа всегда крупнее.
const MIN_SCAN_SIDE: u32 = 700;

/// Ниже этой доли небелых пикселей нарисованная страница считается пустой.
///
/// Замер по присланным файлам: страница выписки — 7.3% небелого, страницы
/// обследований — 9..14%, самый пустой лист (скан айфона) — 2.1%. Пустой даёт
/// ровно 0. Порог стоит в сорок раз ниже самой пустой настоящей страницы:
/// он ловит «нарисовать не вышло», а не «на странице мало текста».
const MIN_INK: f64 = 0.0005;

/// Лист A4 в точках.
const A4_W: f32 = 595.0;
const A4_H: f32 = 842.0;

/// Ориентир для оценки размера: 176 КБ на страницу при JPEG q=55.
const BYTES_PER_PAGE: f64 = 176_000.0;
const CALIBRATED_JPEG: f64 = 55.0;

/// Чем этот документ приходится пакету.
///
/// Нужно ровно для порядка страниц, и порядок этот назвала врач: «первый
/// пункт — 057 направление, второй — выписка, которая формируется в этой
/// программе, третий — обследования, они последние должны быть». Раньше
/// порядок угадывался по имени файла («начинается на выписка», «кончается на
/// .odt»), и угадывание рассыпалось ровно там, где нужнее всего: обследования
/// из КУМки тоже приходят в .odt и становились неотличимы от направления.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Направление 057/у — всегда первым.
    Referral,
    /// Выписка, которую собрала эта программа.
    Vypiska,
    /// Обследования и всё прочее — в конце, в порядке добавления.
    Attachment,
}

/// Одна вложенная вещь: снимок или PDF (может быть многостраничным).
pub struct Source {
    pub name: String,
    pub bytes: Vec<u8>,
    pub kind: Kind,
}

/// Разложить страницы в том порядке, в каком их ждёт принимающая сторона.
///
/// Устойчивая сортировка: внутри одного вида порядок остаётся тот, в каком
/// врач добавляла документы, и её перестановки стрелками не переигрываются.
pub fn in_order(sources: &mut [Source]) {
    sources.sort_by_key(|s| s.kind);
}

/// Где искать LibreOffice.
///
/// На «Альте» 10.4 два имени НЕ равнозначны, и порядок здесь не вкусовой.
/// `/usr/bin/libreoffice` отдаёт пакет `LibreOffice-still-common` — он
/// приезжает с любой сборкой LibreOffice; `/usr/bin/soffice` появляется только
/// вместе с `LibreOffice-integrated`, то есть может не стоять вовсе. Проверено
/// в репозитории p10: `apt-cache showpkg LibreOffice-still-common` показывает
/// `Provides: /usr/bin/libreoffice`, а за `/usr/bin/soffice` apt тянет
/// отдельный пакет. Поэтому первым идёт `libreoffice`.
///
/// Сначала голые имена — их ищут по PATH; следом полные пути: программу
/// запускают из меню рабочего стола, и PATH там бывает урезанным.
#[cfg(unix)]
const OFFICE: [&str; 4] = [
    "libreoffice",
    "soffice",
    "/usr/bin/libreoffice",
    "/usr/bin/soffice",
];

/// На Windows имя одно, а каталог зависит от разрядности сборки офиса.
#[cfg(windows)]
const OFFICE: [&str; 3] = [
    "soffice.exe",
    r"C:\Program Files\LibreOffice\program\soffice.exe",
    r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
];

/// Это направление из КУМки (документ ODF)?
///
/// Стандарт ODF требует, чтобы запись `mimetype` лежала в архиве первой и без
/// сжатия, — поэтому тип читается прямо из байтов, без разбора ZIP: первые
/// 30 байт это заголовок записи, дальше её имя и сразу содержимое.
pub fn is_odt(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK")
        && bytes
            .get(30..)
            .is_some_and(|tail| tail.starts_with(b"mimetypeapplication/vnd.oasis.opendocument"))
}

/// Взять файл с диска и приготовить его к пакету.
///
/// Единственная дверь в пакет: снимок и PDF проходят как есть, направление
/// .odt по дороге переводится в PDF. Раньше двери не было вовсе — направление
/// уходило на вкладку «Выписка» источником данных для полей, в список страниц
/// не попадало, и врач узнавала об этом по готовому файлу, где осталась одна
/// выписка.
pub fn source_from_file(path: &Path, kind: Kind) -> Result<Source> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let bytes = std::fs::read(path).with_context(|| format!("не прочитать {}", path.display()))?;
    source_from_bytes(name, bytes, kind)
}

/// То же, но из уже прочитанных байтов — когда файл по дороге правится
/// (в направление ставятся печать и подпись, см. `odt::with_ink`).
pub fn source_from_bytes(name: String, bytes: Vec<u8>, kind: Kind) -> Result<Source> {
    if is_odt(&bytes) {
        let pdf = odt_to_pdf(&name, &bytes)?;
        return Ok(Source {
            name,
            bytes: pdf,
            kind,
        });
    }
    Ok(Source { name, bytes, kind })
}

/// Аргумент LibreOffice со своим профилем: путь адресом file://, всё не-ASCII
/// закодировано процентами (почему — в `odt_to_pdf`).
fn profile_arg(dir: &Path) -> String {
    let path = dir.display().to_string().replace('\\', "/");
    let mut arg = String::from("-env:UserInstallation=file:///");
    for b in path.trim_start_matches('/').bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.:~".contains(&b) {
            arg.push(b as char);
        } else {
            arg.push_str(&format!("%{b:02X}"));
        }
    }
    arg
}

/// Направление .odt в PDF чужими руками, через LibreOffice.
///
/// Почему именно так, а не иначе:
///
/// * Свою вёрстку бланка 057/у рисовать нельзя. В документе из КУМки стоит
///   отметка об электронной подписи — блок «Сертификат / Владелец /
///   Действителен с … по …» с номером сертификата; в разобранном файле он
///   лежит обычным текстом в `content.xml`, и `odt.rs` его не читает вовсе.
///   Своя вёрстка либо потеряет эту отметку, либо перепечатает чужой номер
///   сертификата на бланке, который нарисовали мы. Второе — подделка
///   официального документа, и обсуждать тут нечего.
/// * Картинку-предпросмотр из самого .odt (`Thumbnails/thumbnail.png`) взять
///   тоже нельзя: замер на живом направлении — 362x512 px, втрое ниже
///   `MIN_WIDTH`. Номер сертификата на такой ширине не читается, а ради него
///   порог и заведён.
///
/// Остаётся отдать перевод тому, кто этот формат и придумал. На рабочем месте
/// врача LibreOffice есть — она этими .odt пользуется каждый день; но «есть
/// обычно» не значит «есть всегда», поэтому отсутствие программы — не паника,
/// а внятный отказ с указанием, что сделать руками.
// ponytail: перевод идёт синхронно, и на первом запуске LibreOffice это
// секунды — окно на это время замирает. Начнёт мешать — унести в поток, как
// сделан приём снимков с телефона (`intake`), и забирать результат каналом.
fn odt_to_pdf(name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    // Отдельный каталог на перевод и удаление даже при ошибке LibreOffice:
    // направление содержит данные пациента и не должно оставаться во временной папке.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0.join("napravlenie.odt"));
            let _ = std::fs::remove_file(self.0.join("napravlenie.pdf"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce)
        .map_err(|e| anyhow::anyhow!("не создать временный каталог: {e}"))?;
    let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("vypiska-057-{}-{suffix}", std::process::id()));
    std::fs::create_dir(&dir)
        .with_context(|| format!("не создать временный каталог {}", dir.display()))?;
    let _cleanup = Cleanup(dir.clone());
    // Имя латиницей и без пробелов: путь уходит в чужую программу, а «й» в
    // именах файлов на диске записана разложенной (NFD) — лишний повод для
    // промаха на ровном месте. Имя выходного файла LibreOffice делает сам:
    // то же самое, но с .pdf.
    let src = dir.join("napravlenie.odt");
    let out = dir.join("napravlenie.pdf");
    // Прошлый результат убираем до запуска: иначе не отличить «перевёл» от
    // «не сделал ничего, а файл лежит с прошлого раза» — и в пакет ушло бы
    // направление на другого ребёнка.
    let _ = std::fs::remove_file(&out);
    std::fs::write(&src, bytes).with_context(|| format!("не записать {}", src.display()))?;

    // Свой профиль обязателен. Без него headless-запуск ничего не делает, пока
    // у врача открыто окно LibreOffice: профиль занят первой копией.
    //
    // Значение уходит адресом file://, а в адресе всё, кроме латиницы, цифр и
    // пары знаков, полагается кодировать процентами. Своё имя каталога у нас
    // латиницей, но временная папка — нет: на Windows она лежит в профиле
    // пользователя, а в поликлиниках учётные записи сплошь и рядом русскими
    // буквами (`C:\Users\Тестовый\…`). Без кодирования LibreOffice не создаёт
    // профиль и молча не переводит ничего.
    let profile = profile_arg(&dir.join("lo-profile"));

    let mut trouble = String::new();
    for exe in OFFICE {
        let run = std::process::Command::new(exe)
            .arg(&profile)
            .args([
                "--headless",
                "--norestore",
                "--convert-to",
                "pdf",
                "--outdir",
            ])
            .arg(&dir)
            .arg(&src)
            .output();
        match run {
            // Нет такой программы — пробуем следующее имя.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => trouble = e.to_string(),
            Ok(done) => {
                // LibreOffice охотно возвращает нулевой код, не сделав ничего,
                // поэтому верим файлу, а не коду возврата.
                if let Ok(pdf) = std::fs::read(&out) {
                    if pdf.starts_with(b"%PDF") {
                        let _ = std::fs::remove_file(&src);
                        let _ = std::fs::remove_file(&out);
                        return Ok(pdf);
                    }
                }
                let said = String::from_utf8_lossy(&done.stderr);
                let said = said.trim();
                trouble = if said.is_empty() {
                    format!("код возврата {}", done.status)
                } else {
                    said.to_string()
                };
            }
        }
    }

    if trouble.is_empty() {
        bail!(
            "«{name}» — документ из КУМки (.odt), а в пакет идут снимки и PDF. \
             Перевести его в PDF некому: на этом компьютере не нашлось LibreOffice. \
             Откройте документ, сохраните его в PDF («Файл» → «Экспорт в PDF» \
             или печать в PDF) и перетащите в пакет уже PDF."
        );
    }
    bail!(
        "«{name}» не перевести в PDF: LibreOffice ответил «{trouble}». \
         Откройте документ сами, сохраните его в PDF и перетащите в пакет уже PDF."
    );
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quality {
    Small,
    Normal,
    Sharp,
}

impl Quality {
    /// Ширина страницы в пикселях.
    pub const fn width(self) -> u32 {
        match self {
            Quality::Small => MIN_WIDTH,
            Quality::Normal => 1400,
            Quality::Sharp => 1600,
        }
    }

    pub fn jpeg(self) -> u8 {
        match self {
            Quality::Small => 45,
            Quality::Normal => 55,
            Quality::Sharp => 70,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Small => "мельче",
            Quality::Normal => "обычное",
            Quality::Sharp => "чётче",
        }
    }
}

/// Ширина, на которой рисуется векторная страница.
///
/// Самая крупная из качеств: `fit` умеет только уменьшать, и нарисуй мы уже —
/// «чётче» перестало бы быть чётче. Обязана оставаться не ниже `MIN_WIDTH`.
const RENDER_WIDTH: u32 = Quality::Sharp.width();

/// Сколько страниц получится и сколько это примерно весит — для показа до сборки.
pub fn estimate(sources: &[Source], q: Quality) -> (usize, u64) {
    let pages: usize = sources.iter().map(page_count).sum();
    // Вес страницы растёт как площадь (ширина в квадрате) и примерно линейно по
    // качеству. Это прикидка для надписи в окне; точный размер знает только build.
    let per_page = BYTES_PER_PAGE
        * (q.width() as f64 / MIN_WIDTH as f64).powi(2)
        * (q.jpeg() as f64 / CALIBRATED_JPEG);
    (pages, (pages as f64 * per_page) as u64)
}

/// Собрать один PDF: каждая страница — JPEG на листе A4.
pub fn build(sources: &[Source], q: Quality, target_bytes: Option<u64>) -> Result<Vec<u8>> {
    if sources.is_empty() {
        bail!("нечего собирать: не добавлено ни одного файла");
    }

    // Разворачиваем и ужимаем один раз, дальше только перекодируем: повторный
    // разбор шести фотографий ради каждой попытки уложиться в вес — это секунды.
    // ponytail: все страницы держатся в памяти разом — это ~7 МБ на лист, то есть
    // сотня мегабайт на большой пакет. Упрётся — разбирать источник заново на
    // каждую попытку качества (медленнее, но памяти почти не ест).
    let mut sheets = Vec::new();
    for source in sources {
        for page in pages(source)? {
            sheets.push(fit(page, q.width()));
        }
    }
    if sheets.is_empty() {
        bail!("в выбранных файлах не нашлось ни одной страницы");
    }

    let mut jpeg = q.jpeg();
    loop {
        let pdf = assemble(&sheets, jpeg)?;
        let too_heavy = target_bytes.is_some_and(|limit| pdf.len() as u64 > limit);
        if !too_heavy || jpeg <= MIN_JPEG {
            // Не уложились даже на минимальном качестве — отдаём что вышло.
            // Тяжёлый пакет врач хотя бы отправит, а отсутствие пакета — нет.
            return Ok(pdf);
        }
        jpeg = jpeg.saturating_sub(5).max(MIN_JPEG);
    }
}

/// Разложить источник на отдельные страницы-картинки (для миниатюр в интерфейсе).
pub fn pages(source: &Source) -> Result<Vec<RgbImage>> {
    if source.bytes.starts_with(b"%PDF") {
        return pdf_pages(source);
    }
    // Направление, попавшее в список неразобранным. Дверь в пакет одна —
    // `source_from_file`, она переводит .odt в PDF, — но раз уж такой файл
    // здесь оказался, сборка обязана встать со словами. Собрать пакет «без
    // одного файла» нельзя: врач узнает о пропаже по готовому документу,
    // который уже ушёл в другое учреждение.
    if is_odt(&source.bytes) {
        bail!(
            "«{}» лежит в списке как .odt — такой файл в PDF не вкладывается. \
             Уберите его из списка и добавьте заново: программа переведёт \
             направление в PDF сама.",
            source.name
        );
    }
    let img = image::load_from_memory(&source.bytes).with_context(|| {
        format!(
            "«{}» — не снимок и не PDF, такой файл в пакет не вложить. \
             Распечатайте его и сфотографируйте, либо сохраните в PDF и \
             добавьте PDF.",
            source.name
        )
    })?;
    Ok(vec![img.to_rgb8()])
}

/// Страницы бывают двух пород, и порода решает способ.
///
/// У скана (телефон, сканер айфона, чужая «сжималка») на листе лежит готовый
/// растр — его достаём и лишний раз не пережимаем. У векторной страницы
/// (выписка чужой клиники, распечатанная в PDF, а не отсканированная) такого
/// растра нет вовсе: есть шрифты и линии, и страницу надо нарисовать.
/// Породу различает тот же признак, что и раньше, — нашёлся ли на листе растр
/// размером со страницу (`MIN_SCAN_SIDE`).
fn pdf_pages(source: &Source) -> Result<Vec<RgbImage>> {
    let doc = Document::load_mem(&source.bytes)
        .with_context(|| format!("«{}» не читается — файл повреждён", source.name))?;

    let mut out: Vec<Option<RgbImage>> = doc
        .get_pages()
        .into_values()
        .map(|page_id| page_image(&doc, page_id))
        .collect();
    if out.is_empty() {
        bail!("в «{}» нет ни одной страницы", source.name);
    }
    // Рисовать заводим, только если доставать нечего: скан разбирается мгновенно,
    // а рисование отняло бы и секунды, и чёткость.
    if out.iter().any(Option::is_none) {
        draw_pages(source, &mut out)?;
    }
    out.into_iter()
        .map(|page| page.context("страница не собралась"))
        .collect()
}

/// Дорисовать те страницы, на которых растра не нашлось.
///
/// Рисует `hayro` — растеризатор PDF на чистом Rust. Выбран не за красоту, а за
/// то, что программа обязана остаться одним exe: pdfium тянет за собой DLL,
/// mupdf — C-тулчейн на машине врача. Вдобавок `hayro` уже лежал в дереве
/// зависимостей: им `typst-render` рисует PDF, вложенные в документ, — так что
/// ни новых крейтов, ни веса это не добавило.
// ponytail: страница рисуется на каждый вызов pages(), в том числе ради
// миниатюр в окне. Начнёт ощущаться ожидание — кэшировать по имени файла.
fn draw_pages(source: &Source, pages: &mut [Option<RgbImage>]) -> Result<()> {
    let pdf = Pdf::new(source.bytes.clone())
        .map_err(|e| anyhow::anyhow!("«{}» не открывается на отрисовку: {e:?}", source.name))?;
    // lopdf и hayro считают страницы каждый по-своему (битое дерево страниц,
    // поиск перебором). Разошлись — неизвестно, какую страницу рисовать вместо
    // какой, и собирать пакет уже нельзя: в другое учреждение уйдёт чужой лист.
    if pdf.pages().len() != pages.len() {
        bail!(
            "«{}»: страницы не сходятся — {} против {}. Такой файл не вложить, \
             распечатайте его и отсканируйте.",
            source.name,
            pdf.pages().len(),
            pages.len()
        );
    }

    let settings = InterpreterSettings {
        font_resolver: Arc::new(system_font),
        ..Default::default()
    };
    let cache = RenderCache::new();
    for (i, (slot, page)) in pages.iter_mut().zip(pdf.pages().iter()).enumerate() {
        if slot.is_some() {
            continue;
        }
        let img = draw(page, &cache, &settings)?;
        // Пустой лист — это не «страница без текста», это «нарисовать не
        // вышло»: шифрование, невиданный шрифт, шрифта нет на машине. Молча
        // вложить его нельзя — пакет уходит в другое учреждение за подписью врача.
        if ink_share(&img) < MIN_INK {
            bail!(
                "«{}», страница {}: нарисовать не удалось, лист вышел пустым. \
                 Распечатайте документ и сфотографируйте его, либо отсканируйте.",
                source.name,
                i + 1
            );
        }
        *slot = Some(img);
    }
    Ok(())
}

/// Один лист в картинку. Поворот `/Rotate` hayro учитывает сам — он уже заложен
/// в `render_dimensions`, поэтому наш `rotate` здесь не нужен.
fn draw<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
) -> Result<RgbImage> {
    let (pt_w, _) = page.render_dimensions();
    let scale = RENDER_WIDTH as f32 / pt_w.max(1.0);
    let pix = hayro::render(
        page,
        cache,
        settings,
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            // Лист белый: под текстом должна быть бумага, а не прозрачность,
            // иначе JPEG положит его на чёрное.
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
            ..Default::default()
        },
    );

    // Пиксели premultiplied — кладём на белое, как это делает вёрстка выписки.
    let rgb: Vec<u8> = pix
        .data()
        .iter()
        .flat_map(|p| {
            let over = |c: u8| c.saturating_add(255 - p.a);
            [over(p.r), over(p.g), over(p.b)]
        })
        .collect();
    RgbImage::from_raw(pix.width() as u32, pix.height() as u32, rgb)
        .context("не сложить нарисованную страницу в картинку")
}

/// Доля небелых пикселей — по ней видно, нарисовалось ли хоть что-нибудь.
fn ink_share(img: &RgbImage) -> f64 {
    let n = img.width() as f64 * img.height() as f64;
    if n == 0.0 {
        return 0.0;
    }
    img.pixels()
        .filter(|p| p.0.iter().any(|&c| c < 240))
        .count() as f64
        / n
}

/// Шрифт, которого в самом PDF нет.
///
/// Документ вправе сослаться на «Times New Roman», не вложив его файл, — тогда
/// шрифт спрашивают у нас. Отдаём системный: ровно так же выписка берёт Times
/// для своего бланка. На Windows это сам Times/Arial/Courier из папки
/// шрифтов; на Linux их не существует, и в дело идёт метрический близнец —
/// Liberation Serif/Sans/Mono (пакет `fonts-ttf-liberation` на «Альте»,
/// `fonts-liberation` на Debian). Вшивать свои копии незачем — это вес в exe,
/// который носят на флешке, а без шрифта страница выйдет пустой, и её
/// завернёт проверка на пустой лист.
fn system_font(query: &FontQuery) -> Option<(FontData, u32)> {
    let file = font_file_name(match query {
        FontQuery::Standard(s) => *s,
        FontQuery::Fallback(f) => f.pick_standard_font(),
    })?;
    let data = std::fs::read(crate::find_font(file)?).ok()?;
    Some((Arc::new(data) as FontData, 0))
}

#[cfg(windows)]
fn font_file_name(font: StandardFont) -> Option<&'static str> {
    Some(match font {
        StandardFont::TimesRoman => "times.ttf",
        StandardFont::TimesBold => "timesbd.ttf",
        StandardFont::TimesItalic => "timesi.ttf",
        StandardFont::TimesBoldItalic => "timesbi.ttf",
        StandardFont::Helvetica => "arial.ttf",
        StandardFont::HelveticaBold => "arialbd.ttf",
        StandardFont::HelveticaOblique => "ariali.ttf",
        StandardFont::HelveticaBoldOblique => "arialbi.ttf",
        StandardFont::Courier => "cour.ttf",
        StandardFont::CourierBold => "courbd.ttf",
        StandardFont::CourierOblique => "couri.ttf",
        StandardFont::CourierBoldOblique => "courbi.ttf",
        // Значковые шрифты: в медицинских бумагах их не бывает, а подменять их
        // текстовым — значит насыпать в документ случайных букв.
        StandardFont::Symbol | StandardFont::ZapfDingBats => return None,
    })
}

#[cfg(unix)]
fn font_file_name(font: StandardFont) -> Option<&'static str> {
    Some(match font {
        StandardFont::TimesRoman => "LiberationSerif-Regular.ttf",
        StandardFont::TimesBold => "LiberationSerif-Bold.ttf",
        StandardFont::TimesItalic => "LiberationSerif-Italic.ttf",
        StandardFont::TimesBoldItalic => "LiberationSerif-BoldItalic.ttf",
        StandardFont::Helvetica => "LiberationSans-Regular.ttf",
        StandardFont::HelveticaBold => "LiberationSans-Bold.ttf",
        StandardFont::HelveticaOblique => "LiberationSans-Italic.ttf",
        StandardFont::HelveticaBoldOblique => "LiberationSans-BoldItalic.ttf",
        StandardFont::Courier => "LiberationMono-Regular.ttf",
        StandardFont::CourierBold => "LiberationMono-Bold.ttf",
        StandardFont::CourierOblique => "LiberationMono-Italic.ttf",
        StandardFont::CourierBoldOblique => "LiberationMono-BoldItalic.ttf",
        StandardFont::Symbol | StandardFont::ZapfDingBats => return None,
    })
}

/// Со страницы берём самый крупный вложенный снимок: у сканов и «сжималок» на
/// листе ровно одна фотография, а мелочь вроде логотипа страницей быть не может.
///
/// `None` здесь означает «это не скан»: такую страницу дорисует `draw_pages`.
// ponytail: у скана снимок достаётся из ресурсов, а не рисуется — так он не
// теряет чёткость на лишнем пережатии и разбирается мгновенно. Поворот и
// масштаб из потока содержимого не разбираются: учитывается только /Rotate.
fn page_image(doc: &Document, page_id: ObjectId) -> Option<RgbImage> {
    let (own, inherited) = doc.get_page_resources(page_id).ok()?;
    let mut resources: Vec<&Dictionary> = own.into_iter().collect();
    resources.extend(
        inherited
            .iter()
            .filter_map(|id| doc.get_dictionary(*id).ok()),
    );

    let mut best: Option<(i64, &Stream)> = None;
    for res in resources {
        let Ok(xobjects) = res
            .get(b"XObject")
            .and_then(|o| doc.dereference(o))
            .and_then(|(_, o)| o.as_dict())
        else {
            continue;
        };
        for (_, value) in xobjects.iter() {
            let Ok(stream) = doc.dereference(value).and_then(|(_, o)| o.as_stream()) else {
                continue;
            };
            if !matches!(stream.dict.get(b"Subtype"), Ok(Object::Name(n)) if n == b"Image") {
                continue;
            }
            let area = pixel_area(&stream.dict);
            if best.is_none_or(|(biggest, _)| area > biggest) {
                best = Some((area, stream));
            }
        }
    }

    let (_, stream) = best?;
    let img = decode(stream)?;
    // Векторная страница (выписка, направление из МИС) тоже содержит картинки —
    // печать, подпись, логотип. Взять самую крупную из них и выдать за страницу
    // нельзя: в пакет уходил оттиск, растянутый на весь лист. У настоящего скана
    // страница всегда крупная.
    if img.width() < MIN_SCAN_SIDE || img.height() < MIN_SCAN_SIDE {
        return None;
    }
    Some(rotate(img, rotation(doc, page_id)))
}

fn pixel_area(dict: &Dictionary) -> i64 {
    let num = |key: &[u8]| match dict.get(key) {
        Ok(Object::Integer(i)) => *i,
        _ => 0,
    };
    num(b"Width").saturating_mul(num(b"Height"))
}

/// Снимок внутри PDF лежит так, как снят, а поворот листа хранится отдельно.
/// Без него страница, отсканированная боком, уедет в пакет боком.
// ponytail: /Rotate читаем только со самой страницы; наследование от узла Pages
// у сканов не встречается, а обход дерева — лишний код.
fn rotation(doc: &Document, page_id: ObjectId) -> i64 {
    doc.get_dictionary(page_id)
        .and_then(|d| d.get(b"Rotate"))
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .rem_euclid(360)
}

fn rotate(img: RgbImage, degrees: i64) -> RgbImage {
    match degrees {
        90 => image::imageops::rotate90(&img),
        180 => image::imageops::rotate180(&img),
        270 => image::imageops::rotate270(&img),
        _ => img,
    }
}

fn decode(stream: &Stream) -> Option<RgbImage> {
    let filters: Vec<Vec<u8>> = stream
        .filters()
        .map(|names| names.iter().map(|n| n.to_vec()).collect())
        .unwrap_or_default();

    // Фильтры идут в порядке разворачивания, кодек картинки — всегда последний.
    let (wrappers, codec) = match filters.split_last() {
        Some((last, head)) if is_image_codec(last) => (head, Some(last.as_slice())),
        _ => (filters.as_slice(), None),
    };

    let data = if wrappers.is_empty() {
        stream.content.clone()
    } else {
        // «Сжималки» кладут JPEG ещё и под Flate: /Filter [/FlateDecode /DCTDecode].
        // lopdf на DCTDecode сдаётся и отдаёт ошибку вместо уже развёрнутых данных,
        // поэтому подсовываем ему копию потока без кодека картинки.
        let mut dict = stream.dict.clone();
        dict.set(
            "Filter",
            Object::Array(wrappers.iter().map(|n| Object::Name(n.clone())).collect()),
        );
        Stream::new(dict, stream.content.clone())
            .decompressed_content()
            .ok()?
    };

    match codec {
        // Сканер айфона кладёт сырые сэмплы под Flate, а не JPEG.
        None => raw_samples(&stream.dict, data),
        Some(name) => match name {
            b"DCTDecode" => image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg)
                .ok()
                .map(|img| img.to_rgb8()),
            b"JPXDecode" => image::load_from_memory(&data).ok().map(|img| img.to_rgb8()),
            // Факс и JBIG2 — из мира офисных МФУ, в присланных файлах их нет.
            _ => None,
        },
    }
}

fn is_image_codec(name: &[u8]) -> bool {
    matches!(
        name,
        b"DCTDecode" | b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode"
    )
}

/// Растр без собственного заголовка: размеры есть в словаре, а число каналов
/// считаем из объёма данных — так не нужно разворачивать ColorSpace, который в
/// этих файлах лежит отдельным объектом (ICCBased).
fn raw_samples(dict: &Dictionary, mut data: Vec<u8>) -> Option<RgbImage> {
    let num = |key: &[u8]| match dict.get(key) {
        Ok(Object::Integer(i)) => u32::try_from(*i).ok(),
        _ => None,
    };
    let (w, h) = (num(b"Width")?, num(b"Height")?);
    if w == 0 || h == 0 || num(b"BitsPerComponent").unwrap_or(8) != 8 {
        return None;
    }

    let px = (w as usize).checked_mul(h as usize)?;
    let channels = data.len().checked_div(px)?;
    data.truncate(px.checked_mul(channels)?);

    match channels {
        3 => RgbImage::from_raw(w, h, data),
        1 => {
            Some(image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, data)?).to_rgb8())
        }
        4 => {
            // CMYK -> RGB, наивно, но для сканов этого достаточно.
            let rgb: Vec<u8> = data
                .chunks_exact(4)
                .flat_map(|c| {
                    let k = c[3] as u32;
                    [0, 1, 2].map(|i| (((255 - c[i] as u32) * (255 - k)) / 255) as u8)
                })
                .collect();
            RgbImage::from_raw(w, h, rgb)
        }
        _ => None,
    }
}

/// Ширина задаёт и вес, и читаемость; высота идёт за ней. Вверх не тянем:
/// из мелкого снимка резкости не появится, появится только вес.
fn fit(img: RgbImage, width: u32) -> RgbImage {
    if img.width() <= width {
        return img;
    }
    image::DynamicImage::ImageRgb8(img)
        .resize(width, u32::MAX, image::imageops::FilterType::Lanczos3)
        .to_rgb8()
}

fn assemble(sheets: &[RgbImage], jpeg_quality: u8) -> Result<Vec<u8>> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::with_capacity(sheets.len());

    for sheet in sheets {
        let (w, h) = sheet.dimensions();
        if w == 0 || h == 0 {
            bail!("одна из страниц пустая — снимок не прочитался");
        }

        let jpeg = encode_jpeg(sheet, jpeg_quality)?;
        let image_id = doc.add_object(
            Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => w as i64,
                    "Height" => h as i64,
                    "ColorSpace" => "DeviceRGB",
                    "BitsPerComponent" => 8,
                    "Filter" => "DCTDecode",
                },
                jpeg,
            )
            .with_compression(false),
        );

        // Вписываем целиком и по центру: у сканов свои поля, и обрезать их —
        // значит рисковать краем печати или подписи.
        let scale = (A4_W / w as f32).min(A4_H / h as f32);
        let (dw, dh) = (w as f32 * scale, h as f32 * scale);
        let (x, y) = ((A4_W - dw) / 2.0, (A4_H - dh) / 2.0);
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("q {dw:.2} 0 0 {dh:.2} {x:.2} {y:.2} cm /Im Do Q").into_bytes(),
        ));

        kids.push(Object::Reference(doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Im" => image_id },
            },
        })));
    }

    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
            // Лист один на весь пакет, книжный A4 — в таком виде больница его и ждёт.
            "MediaBox" => vec![0.into(), 0.into(), A4_W.into(), A4_H.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .context("не удалось записать готовый PDF")?;
    Ok(out)
}

fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        // encode_image берёт размеры у самой картинки; encode() рядом сверяет
        // длину буфера утверждением и падает на расхождении.
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
        encoder
            .encode_image(img)
            .context("не удалось сжать страницу")?;
    }
    Ok(out)
}

fn page_count(source: &Source) -> usize {
    if !source.bytes.starts_with(b"%PDF") {
        return 1;
    }
    // Битый PDF считаем за одну страницу: прикидка показывается ещё до сборки и
    // падать не должна — на настоящую ошибку наткнётся build и объяснит её врачу.
    Document::load_mem(&source.bytes)
        .map(|doc| doc.page_iter().count())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn source(name: &str) -> Source {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        Source {
            kind: Kind::Attachment,
            name: name.into(),
            bytes,
        }
    }

    /// Размеры снимков в собранном пакете — по ним видно и число страниц,
    /// и что ширина не просела.
    fn sheets_of(pdf: &[u8]) -> Vec<(i64, i64)> {
        let doc =
            Document::load_mem(pdf).unwrap_or_else(|e| panic!("собранный PDF не читается: {e}"));
        doc.page_iter()
            .map(|id| {
                doc.get_page_images(id)
                    .unwrap_or_default()
                    .first()
                    .map(|img| (img.width, img.height))
                    .unwrap_or((0, 0))
            })
            .collect()
    }

    #[test]
    fn every_page_of_every_source_is_found() {
        for (name, expected) in [
            ("six-pages.pdf", 6),
            ("five-pages.pdf", 5),
            ("scan.pdf", 1),
            ("stamp-sample.jpg", 1),
        ] {
            let got = pages(&source(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                got.len(),
                expected,
                "{name}: страниц {} вместо {expected}",
                got.len()
            );
            for (i, page) in got.iter().enumerate() {
                assert!(
                    page.width() > 0 && page.height() > 0,
                    "{name}: страница {i} пустая"
                );
            }
        }
    }

    /// Синтетический PDF со снимком отдаёт страницу в полном размере.
    #[test]
    fn synthetic_scan_comes_out_at_full_size() {
        let got = pages(&source("scan.pdf")).unwrap_or_else(|e| panic!("{e}"));
        let Some(page) = got.first() else {
            panic!("скан не разобрался");
        };
        assert_eq!(page.dimensions(), (1506, 1555));
    }

    #[test]
    fn package_is_a_pdf_and_keeps_the_order_of_sources() {
        let pdf = build(
            &[source("scan.pdf"), source("stamp-sample.jpg")],
            Quality::Normal,
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(pdf.starts_with(b"%PDF"), "на выходе не PDF");
        let sheets = sheets_of(&pdf);
        assert_eq!(sheets.len(), 2, "страниц {} вместо 2", sheets.len());
        // Скан крупный и ужимается до ширины качества, печать мелкая и остаётся как есть.
        assert_eq!(sheets.first().map(|s| s.0), Some(1400));
        assert_eq!(sheets.get(1).map(|s| s.0), Some(347));
    }

    #[test]
    fn a_pdf_and_a_photo_together_give_the_sum_of_pages() {
        let pdf = build(
            &[source("five-pages.pdf"), source("stamp-sample.jpg")],
            Quality::Normal,
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(sheets_of(&pdf).len(), 6, "5 страниц PDF + снимок");
    }

    /// Главное требование: в вес укладываемся качеством, а не разрешением.
    /// 200 КБ на шесть страниц недостижимы — важно, что ширина всё равно 1240.
    #[test]
    fn a_hard_limit_lowers_quality_but_never_width() {
        let doc = source("six-pages.pdf");
        let loose = build(std::slice::from_ref(&doc), Quality::Small, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let tight = build(std::slice::from_ref(&doc), Quality::Small, Some(200_000))
            .unwrap_or_else(|e| panic!("{e}"));

        let sheets = sheets_of(&tight);
        assert_eq!(sheets.len(), 6);
        for (i, (w, _)) in sheets.iter().enumerate() {
            assert_eq!(*w, MIN_WIDTH as i64, "страница {i}: ширина просела до {w}");
        }
        assert!(
            tight.len() < loose.len(),
            "качество не снизилось: {} против {}",
            tight.len(),
            loose.len()
        );
    }

    /// Калибровка: шесть страниц на 1240 px укладывались в ~1.08 МБ.
    #[test]
    fn six_photographed_pages_stay_light() {
        let pdf = build(&[source("six-pages.pdf")], Quality::Small, None)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            pdf.len() < 1_500_000,
            "пакет потяжелел до {} байт — принимающая сторона такой не откроет",
            pdf.len()
        );
        assert!(
            pdf.len() > 100_000,
            "пакет подозрительно лёгкий: {} байт",
            pdf.len()
        );
    }

    /// Векторный PDF: выписка, распечатанная в файл, а не отсканированная.
    /// Вложенного растра на её страницах нет вовсе.
    fn vector_pdf() -> Source {
        let mut v = crate::vypiska::Vypiska::new();
        v.fio = "Тестов Алексей Примерович".into();
        v.dob = "5 мая 2016 года".into();
        v.doctor = "Примерова П. П.".into();
        v.diagnosis = "Z13.8".into();
        v.complaints = "нет".into();
        v.chdd = "20".into();
        v.chss = "80".into();
        v.sign = Some(
            std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/sign-sample.png"
            ))
            .expect("синтетическое изображение подписи лежит в fixtures"),
        );
        Source {
            kind: Kind::Attachment,
            name: "выписка.pdf".into(),
            bytes: crate::vypiska::to_pdf(v).expect("синтетическая выписка собирается"),
        }
    }

    /// Векторная страница содержит вложенную картинку подписи. Пакет должен
    /// взять весь лист, а не растянуть эту картинку до размера страницы.
    #[test]
    fn a_text_pdf_is_drawn_whole_and_not_mistaken_for_its_image() {
        let src = vector_pdf();
        let got = pages(&src).unwrap_or_else(|e| panic!("{e:#}"));
        let page = got.first().expect("страница выписки должна нарисоваться");

        // Ширину в RENDER_WIDTH может дать только нарисованный лист.
        assert_eq!(
            page.width(),
            RENDER_WIDTH,
            "ширина {} — это не лист",
            page.width()
        );
        assert!(page.width() >= MIN_WIDTH);
        // Книжный A4: печать круглая, её соотношение сторон около единицы.
        let ratio = page.width() as f64 / page.height() as f64;
        assert!((0.68..=0.73).contains(&ratio), "это не лист A4: {ratio:.2}");
        // И на листе действительно есть текст: пустой дал бы ровно 0.
        let ink = ink_share(page);
        assert!(
            ink > MIN_INK * 10.0,
            "лист почти пустой: {:.3}%",
            ink * 100.0
        );
    }

    #[test]
    fn nothing_to_pack_is_a_readable_error() {
        let Err(e) = build(&[], Quality::Normal, None) else {
            panic!("пустой список собрался в пакет");
        };
        assert!(
            e.to_string().contains("ни одного файла"),
            "непонятная ошибка: {e}"
        );
    }

    /// Путь к синтетическому направлению.
    fn napravlenie() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/referral.odt")
    }

    /// Направление узнаётся по байтам, а всё остальное — нет. Ошибка в любую
    /// сторону дорогая: ложное «да» отправит снимок в LibreOffice, ложное
    /// «нет» вернёт ту самую пропажу направления из пакета.
    #[test]
    fn a_referral_is_told_apart_from_a_photo_and_a_pdf() {
        let odt = std::fs::read(napravlenie()).unwrap_or_else(|e| panic!("{e}"));
        assert!(is_odt(&odt), "настоящее направление не опознано");
        assert!(
            !is_odt(&source("stamp-sample.jpg").bytes),
            "снимок принят за направление"
        );
        assert!(
            !is_odt(&source("six-pages.pdf").bytes),
            "PDF принят за направление"
        );
        assert!(!is_odt(b""), "пустые байты приняты за направление");
        assert!(!is_odt(b"PK\x03\x04"), "любой ZIP принят за направление");
    }

    /// Главная защита: неподдерживаемый файл НЕ исчезает молча.
    ///
    /// Именно это и случилось у врача — направление не попало в пакет, и
    /// пакет всё равно собрался. Сборка обязана встать и назвать файл.
    /// На Windows временная папка лежит в профиле пользователя, а учётные
    /// записи в поликлиниках бывают русскими буквами. Незакодированную
    /// кириллицу в адресе file:// LibreOffice не принимает и молча ничего не
    /// переводит.
    #[test]
    fn the_office_profile_path_is_percent_encoded() {
        let arg = profile_arg(Path::new(r"C:\Users\Тестовый\Temp\lo-profile"));
        assert!(
            arg.starts_with("-env:UserInstallation=file:///C:/Users/"),
            "{arg}"
        );
        assert!(arg.is_ascii(), "в адресе осталась кириллица: {arg}");
        // «Т» в UTF-8 — это D0 A2.
        assert!(arg.contains("%D0%A2"), "{arg}");
        assert!(arg.ends_with("/Temp/lo-profile"), "{arg}");
        // Путь на Linux не портится.
        assert_eq!(
            profile_arg(Path::new("/tmp/vypiska-057-1/lo-profile")),
            "-env:UserInstallation=file:///tmp/vypiska-057-1/lo-profile"
        );
    }

    /// Порядок страниц назвала врач: «первый пункт — 057 направление,
    /// второй — выписка, которая формируется в этой программе, третий — вот
    /// эти обследования, они последние должны быть». Складывает она их в
    /// другом порядке — обследования по ходу приёма, выписку в самом конце.
    #[test]
    fn the_referral_leads_the_package_and_the_tests_close_it() {
        let mk = |name: &str, kind: Kind| Source {
            name: name.into(),
            bytes: b"x".to_vec(),
            kind,
        };
        let mut p = vec![
            mk("узи.odt", Kind::Attachment),
            mk("кровь.odt", Kind::Attachment),
            mk("выписка, стр. 1", Kind::Vypiska),
            mk("лапкова 057у.odt", Kind::Referral),
        ];
        in_order(&mut p);
        let names: Vec<&str> = p.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "лапкова 057у.odt",
                "выписка, стр. 1",
                "узи.odt",
                "кровь.odt"
            ],
            "порядок не тот, который просила врач"
        );
    }

    #[test]
    fn an_unsupported_file_never_disappears_quietly() {
        let odt = std::fs::read(napravlenie()).unwrap_or_else(|e| panic!("{e}"));
        let strays = [
            Source {
                name: "направление.odt".into(),
                bytes: odt,
                kind: Kind::Referral,
            },
            Source {
                name: "заметка.txt".into(),
                bytes: "просто текст".as_bytes().to_vec(),
                kind: Kind::Attachment,
            },
        ];
        for stray in strays {
            let name = stray.name.clone();
            let Err(e) = pages(&stray) else {
                panic!("«{name}» разобрался в страницы, хотя не должен");
            };
            let said = format!("{e:#}");
            assert!(said.contains(&name), "ошибка не называет файл: {said}");

            // И то же самое при сборке всего пакета: не «на одну страницу
            // меньше», а отказ. Рядом кладём заведомо годный снимок — он не
            // должен служить оправданием, чтобы промолчать про второй файл.
            let Err(e) = build(&[source("stamp-sample.jpg"), stray], Quality::Small, None) else {
                panic!("пакет собрался без «{name}» и промолчал об этом");
            };
            assert!(
                format!("{e:#}").contains(&name),
                "пакет промолчал про «{name}»"
            );
        }
    }

    /// Направление .odt должно попасть в пакет — либо переведённым в PDF,
    /// либо внятным отказом, из которого видно, что делать руками.
    ///
    /// Оба исхода законные: LibreOffice есть не на каждой машине. Молчания и
    /// невнятицы среди исходов нет.
    #[test]
    fn a_referral_becomes_a_pdf_or_says_why_it_cannot() {
        match source_from_file(&napravlenie(), Kind::Referral) {
            Ok(src) => {
                assert!(src.bytes.starts_with(b"%PDF"), "на выходе не PDF");
                let got = pages(&src).unwrap_or_else(|e| panic!("переведённое не читается: {e:#}"));
                assert!(!got.is_empty(), "в переведённом направлении нет страниц");
                for page in &got {
                    assert!(
                        page.width() >= MIN_WIDTH,
                        "страница шириной {} — номер сертификата не прочитать",
                        page.width()
                    );
                }
            }
            Err(e) => {
                let said = format!("{e:#}");
                assert!(said.contains("LibreOffice"), "непонятный отказ: {said}");
                assert!(
                    said.contains("PDF"),
                    "в отказе не сказано, что делать: {said}"
                );
            }
        }
    }

    /// Снимок и PDF идут в пакет как есть — перевод их не касается.
    #[test]
    fn a_photo_goes_into_the_package_untouched() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stamp-sample.jpg");
        let src = source_from_file(&path, Kind::Attachment).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(src.bytes, std::fs::read(&path).unwrap_or_default());
        assert_eq!(src.name, "stamp-sample.jpg", "потерялось имя файла");
    }

    #[test]
    fn estimate_counts_every_page_before_the_build() {
        let sources = [source("six-pages.pdf"), source("stamp-sample.jpg")];
        let (pages, bytes) = estimate(&sources, Quality::Normal);
        assert_eq!(pages, 7);
        assert!(bytes > 0);
        // Чем чётче, тем тяжелее — иначе выбор качества ни о чём не говорит.
        assert!(estimate(&sources, Quality::Sharp).1 > estimate(&sources, Quality::Small).1);
    }
}
