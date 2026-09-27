//! Разбор направления (форма 057/у), как его отдаёт qMS: ODT = ZIP + content.xml.
//!
//! Вся форма — одна таблица, и в ней действует одно правило:
//! подпись поля лежит голым текстом, значение — внутри <text:span>.
//! Поэтому ничего не парсится регулярками по расплющенному тексту.

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;

/// Абзац: весь текст и отдельно то, что лежало в <text:span>.
#[derive(Debug, Default, Clone)]
pub struct Para {
    pub full: String,
    pub spans: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Dob {
    pub day: u32,
    pub month: String,
    pub year: i32,
}

#[derive(Debug, Default, Clone)]
pub struct Napravlenie {
    pub number: String,
    pub date: String,
    pub target_mo: String,
    pub policy: String,
    pub policy_date: String,
    pub insurer: String,
    pub fio: String,
    pub dob: Dob,
    pub sex: String,
    pub addr_reg: String,
    pub addr_stay: String,
    pub diagnosis: String,
    pub reason: String,
    pub doctor: String,
    pub doctor_post: String,
    pub head_of_dept: String,
}

impl Napravlenie {
    /// Где ребёнок живёт по факту — в графу «Домашний адрес» формы 027/у.
    ///
    /// Форма 027/у (приказ Минздрава СССР от 04.10.1980 № 1030, применяется
    /// по письму Минздравсоцразвития РФ от 30.11.2009 № 14-6/242888) называет
    /// графу «Домашний адрес» и инструкции по её заполнению не даёт вовсе:
    /// приказ 1980 года — это бланки без порядка ведения. Ответ приходится
    /// брать со стороны направления, а не выписки.
    ///
    /// Новая форма 057/у (приказ Минздрава России от 02.09.2025 № 519н,
    /// действует с 26.10.2025 — это её и выгружает qMS) специально завела
    /// вторую строку адреса. Разборы приказа говорят про неё прямо: «помимо
    /// места регистрации по месту жительства пациента добавили новую строку
    /// с информацией о регистрации по месту пребывания. Для пациентов — это
    /// место фактического жительства», и «допускается указать в обеих графах
    /// тот адрес, который известен и указан в документе, удостоверяющем
    /// личность». Вторая часть объясняет наш единственный живой файл, где обе
    /// строки совпадают до буквы: про пребывание просто не спросили.
    ///
    /// Отсюда правило ниже: пребывание заполнено → оно и есть фактический
    /// адрес; пусто → живёт по прописке. Медкарта стационарного больного
    /// (003/у) требует ровно «адрес фактического проживания», так что
    /// направление и выписка сойдутся.
    ///
    /// ДОПУЩЕНИЕ, а не доказанный факт. Полный текст порядка ведения 057/у
    /// (приложение № 2 к приказу 519н) за платной стеной — процитировать
    /// сам пункт не удалось, только разборы. И закон РФ от 25.06.1993
    /// № 5242-1 (ст. 2) относит к «месту пребывания» и гостиницу, и санаторий,
    /// и больницу, а временную регистрацию оформляют и ради школы или работы,
    /// продолжая жить по прописке. То есть обратный случай законом не
    /// запрещён, он просто редкий.
    ///
    /// Цена ошибки низкая: это подсказка, врач видит адрес в окне и правит
    /// руками. Раньше печаталась прописка молча, и исправить её было нельзя —
    /// что пришло из qMS, то и уходило в другое учреждение.
    ///
    /// Вопрос врачу, снимающий неопределённость одной фразой:
    /// «Если в направлении два РАЗНЫХ адреса — постоянная прописка и
    /// временная регистрация — какой из них ставить в „Домашний адрес“
    /// выписки?»
    pub fn address_by_fact(&self) -> &str {
        if self.addr_stay.trim().is_empty() {
            &self.addr_reg
        } else {
            &self.addr_stay
        }
    }
}

pub fn parse_file(path: &std::path::Path) -> Result<Napravlenie> {
    let file =
        std::fs::File::open(path).with_context(|| format!("не открыть {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .context("это не похоже на направление из qMS — нужен файл .odt")?;
    let mut xml = String::new();
    zip.by_name("content.xml")
        .context("в .odt нет content.xml")?
        .read_to_string(&mut xml)?;
    let n = parse_xml(&xml)?;
    recognised(&n)?;
    Ok(n)
}

/// Похоже ли разобранное на направление вообще.
///
/// Разбор опирается на подписи полей («Фамилия, имя, отчество…»). Стоит qMS
/// обновить бланк — а она уже обновлялась, направление теперь по приказу
/// 519н от 02.09.2025, — и подписи разъедутся: каждое поле молча станет
/// пустым, а файл при этом откроется без единой жалобы.
///
/// Чем это опасно: врач видит «не хватает: ФИО, дата рождения, диагноз» при
/// загруженном файле и думает, что чего-то не дозаполнила, хотя вписать ФИО
/// в программе негде вовсе. Причина — в другом, и сказать о ней надо прямо.
///
/// ФИО берём за признак: в направлении оно есть всегда, пустым не бывает.
/// Проверка живёт в `parse_file`, а не в `parse_xml`: второй зовут тесты на
/// обрывках разметки, где целого направления и не предполагается.
fn recognised(n: &Napravlenie) -> Result<()> {
    if n.fio.trim().is_empty() {
        bail!(
            "файл открылся, но это не похоже на направление: не нашлось ни ФИО, \
             ни других полей. Либо выбран не тот файл, либо в qMS сменился \
             бланк и программу нужно поправить"
        );
    }
    Ok(())
}

pub fn parse_xml(xml: &str) -> Result<Napravlenie> {
    let paras = paragraphs(xml)?;
    Ok(from_paragraphs(&paras))
}

/// Обход content.xml: собираем абзацы и спаны внутри них.
pub fn paragraphs(xml: &str) -> Result<Vec<Para>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut out: Vec<Para> = Vec::new();
    let mut cur: Option<Para> = None;
    let mut span_depth = 0usize;

    loop {
        match reader.read_event()? {
            Event::Start(e) => match e.name().as_ref() {
                "text:p" | "text:h" => {
                    cur = Some(Para::default());
                    span_depth = 0;
                }
                "text:span" => {
                    if span_depth == 0 {
                        if let Some(p) = cur.as_mut() {
                            p.spans.push(String::new());
                        }
                    }
                    span_depth += 1;
                }
                _ => {}
            },
            Event::End(e) => match e.name().as_ref() {
                "text:p" | "text:h" => {
                    if let Some(mut p) = cur.take() {
                        p.full = norm(&p.full);
                        p.spans = p.spans.iter().map(|s| norm(s)).collect();
                        if !p.full.is_empty() {
                            out.push(p);
                        }
                    }
                    span_depth = 0;
                }
                "text:span" => span_depth = span_depth.saturating_sub(1),
                _ => {}
            },
            // <text:s/> — пробел, <text:tab/> — табуляция
            Event::Empty(e) => {
                let t = match e.name().as_ref() {
                    "text:s" | "text:tab" | "text:line-break" => " ",
                    _ => "",
                };
                if !t.is_empty() {
                    push_text(&mut cur, span_depth, t);
                }
            }
            Event::Text(e) => {
                let t = e.xml10_content().into_owned();
                push_text(&mut cur, span_depth, &t);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn push_text(cur: &mut Option<Para>, span_depth: usize, t: &str) {
    if let Some(p) = cur.as_mut() {
        p.full.push_str(t);
        if span_depth > 0 {
            if let Some(s) = p.spans.last_mut() {
                s.push_str(t);
            }
        }
    }
}

/// Неразрывные пробелы и повторы — в обычный пробел, иначе подписи полей не совпадут.
/// `split_whitespace` знает про весь юникодный пробельный класс, включая U+00A0 и U+2007.
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find<'a>(paras: &'a [Para], label: &str) -> Option<&'a Para> {
    paras.iter().find(|p| p.full.starts_with(label))
}

/// Значение поля: кусок абзаца от первого спана до последнего, иначе — хвост
/// после подписи.
///
/// Именно диапазон, а не `spans[0]`: qMS разрывает одно значение на
/// несколько спанов, как только внутри меняется начертание — в дате рождения
/// таких спанов три. Пока бралcя только первый, длинное значение молча
/// обрезалось, и обрезанный адрес выглядел как настоящий.
///
/// Каждый следующий спан ищем ПОСЛЕ конца предыдущего: в графах с вариантами
/// («Пол: муж - 1, жен - 2») тот же текст встречается и в подписи.
///
/// Срез по `label.len()` безопасен: `starts_with` гарантирует побайтовое
/// совпадение префикса, значит смещение попадает на границу символа.
fn val(paras: &[Para], label: &str) -> String {
    let Some(p) = find(paras, label) else {
        return String::new();
    };
    let tail = &p.full[label.len()..];
    let (mut start, mut end) = (None, 0usize);
    for s in p.spans.iter().filter(|s| !s.is_empty()) {
        if let Some(i) = tail[end..].find(s.as_str()) {
            let i = end + i;
            start.get_or_insert(i);
            end = i + s.len();
        }
    }
    match start {
        Some(st) => tail[st..end].trim().to_string(),
        None => tail.trim().to_string(),
    }
}

fn from_paragraphs(paras: &[Para]) -> Napravlenie {
    let mut n = Napravlenie {
        number: val(paras, "НАПРАВЛЕНИЕ ДЛЯ ОКАЗАНИЯ МЕДИЦИНСКОЙ ПОМОЩИ №"),
        date: val(paras, "Дата заполнения направления:"),
        policy: val(paras, "Полис обязательного медицинского страхования:"),
        policy_date: val(
            paras,
            "дата выдачи полиса обязательного медицинского страхования:",
        ),
        insurer: val(paras, "данные о страховой медицинской организации"),
        fio: val(paras, "Фамилия, имя, отчество (при наличии) пациента"),
        sex: val(paras, "Пол:"),
        addr_reg: val(paras, "Регистрация по месту жительства:"),
        addr_stay: val(paras, "Регистрация по месту пребывания:"),
        diagnosis: val(paras, "Код диагноза по Международной"),
        reason: val(paras, "Обоснования (показания) направления"),
        head_of_dept: val(paras, "Заведующий отделением:"),
        ..Default::default()
    };

    // МО назначения — абзац перед «(наименование медицинского учреждения…)»
    if let Some(i) = paras
        .iter()
        .position(|p| p.full.starts_with("(наименование медицинского учреждения"))
    {
        if i > 0 {
            n.target_mo = paras[i - 1].full.clone();
        }
    }

    // Врач: должность голым текстом, ФИО в спане. Привязываемся к подписи графы,
    // а не к слову «Врач» по всему документу — иначе поле мог бы украсть
    // любой более ранний абзац.
    if let Some(i) = paras.iter().position(|p| {
        p.full
            .starts_with("Должность, специальность медицинского работника")
    }) {
        if let Some(p) = paras
            .iter()
            .skip(i + 1)
            .take(3)
            .find(|p| !p.spans.is_empty() && !p.spans[0].is_empty())
        {
            n.doctor = p.spans[0].clone();
            if let Some(post) = p.full.strip_suffix(p.spans[0].as_str()) {
                n.doctor_post = post.trim().trim_end_matches(',').to_string();
            }
        }
    }

    n.dob = dob(paras);
    n
}

/// Год за пределами диапазона оставляем нулём — `fmt::to_date` такую дату
/// отвергнет, и поле уйдёт в `missing()`. Раньше `unwrap_or(0)` давал год 0,
/// который `NaiveDate` считает валидным, и в выписку попадало «0 года (2025 лет)».
fn year_or_zero(s: &str) -> i32 {
    match s.trim().parse::<i32>() {
        Ok(y) if (1900..=2200).contains(&y) => y,
        _ => 0,
    }
}

fn dob(paras: &[Para]) -> Dob {
    let Some(p) = find(paras, "Дата рождения:") else {
        return Dob::default();
    };
    // День, месяц и год могут лежать в трёх спанах подряд.
    if p.spans.len() >= 3 {
        return Dob {
            day: p.spans[0].trim().parse().unwrap_or(0),
            month: p.spans[1].trim().to_lowercase(),
            year: year_or_zero(&p.spans[2]),
        };
    }
    // запасной разбор, если вёрстка другая
    let cleaned: String = p
        .full
        .chars()
        .map(|c| if c == '«' || c == '»' { ' ' } else { c })
        .collect();
    let mut d = Dob::default();
    for w in cleaned.split_whitespace() {
        let w = w
            .trim_end_matches('.')
            .trim_end_matches('г')
            .trim_end_matches('.');
        if let Ok(v) = w.parse::<i32>() {
            if (1..=31).contains(&v) && d.day == 0 {
                d.day = v as u32;
            } else if v > 1900 {
                d.year = year_or_zero(w);
            }
        } else {
            let lw = w.to_lowercase();
            if MONTHS.contains(&lw.as_str()) && d.month.is_empty() {
                d.month = lw;
            }
        }
    }
    d
}

pub const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

/// Поставить в направление печать и подпись — прямо в его .odt, до перевода в
/// PDF.
///
/// Врач: «На направлении тоже печать надо, чтоб была, и подпись, а то только
/// на выписке». Направление — чужой бланк qMS, и рисовать его заново нельзя
/// (в нём отметка об ЭП, см. `package::source_from_file`). Поэтому картинки
/// кладутся в сам документ, а место им находит LibreOffice по вёрстке бланка —
/// не по координатам, которые сломались бы на первом же изменении формы.
///
/// Якоря — надписи самого бланка:
/// - подпись — в пустую ячейку справа от строки врача: это ячейка над первой
///   подписью «Фамилия, имя, отчество (при наличии), подпись» (вторая такая же
///   — у заведующего, его подписи у нас нет, и туда ничего не ставим);
/// - печать — на строку «М.П. (при наличии)».
///
/// Не нашёлся якорь — ошибка, а не молча пропущенная печать: врач должна
/// узнать, что документ ушёл без оттиска, до отправки, а не от краевой.
pub fn with_ink(
    odt: &[u8],
    stamp: Option<&[u8]>,
    stamp_cm: f32,
    sign: Option<&[u8]>,
) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(odt)).context("это не .odt")?;
    let mut content = String::new();
    zip.by_name("content.xml")
        .context("в направлении нет content.xml")?
        .read_to_string(&mut content)?;
    let mut manifest = String::new();
    zip.by_name("META-INF/manifest.xml")
        .context("в направлении нет манифеста")?
        .read_to_string(&mut manifest)?;

    let mut pics: Vec<(&str, &[u8])> = Vec::new();
    if let Some(png) = sign {
        let at = signature_paragraph(&content)
            .context("не нашлась строка подписи врача в направлении — подпись не поставлена")?;
        let frame = ink_frame(
            "Подпись врача",
            "Pictures/vypiska-podpis.png",
            png,
            2.6,
            0.2,
            -0.45,
        )?;
        content = insert_in_paragraph(&content, at, &frame);
        pics.push(("Pictures/vypiska-podpis.png", png));
    }
    if let Some(png) = stamp {
        let at = content
            .find("М.П.")
            .and_then(|i| content[..i].rfind("<text:p"))
            .context("не нашлась строка «М.П.» в направлении — печать не поставлена")?;
        // Печать ставится поверх «М.П.» и чуть выше — туда, где она стоит на
        // бумажном бланке: захватывает строки подписей, но не закрывает их.
        let frame = ink_frame(
            "Печать",
            "Pictures/vypiska-pechat.png",
            png,
            stamp_cm,
            2.2,
            -1.35,
        )?;
        content = insert_in_paragraph(&content, at, &frame);
        pics.push(("Pictures/vypiska-pechat.png", png));
    }
    if pics.is_empty() {
        return Ok(odt.to_vec());
    }

    // Стиль рамки: поверх текста, без обтекания, и не заперта в ячейке
    // таблицы — иначе LibreOffice раздвинул бы строку бланка под картинку и
    // вёрстка направления поехала бы.
    let style = r#"<style:style style:name="vypiskaInk" style:family="graphic"><style:graphic-properties style:wrap="run-through" style:run-through="foreground" style:vertical-pos="from-top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="paragraph" style:flow-with-text="false" fo:border="none" fo:padding="0cm"/></style:style>"#;
    let styles_at = content
        .find("<office:automatic-styles>")
        .map(|i| i + "<office:automatic-styles>".len())
        .context("в направлении нет раздела стилей")?;
    content.insert_str(styles_at, style);

    let entries: String = pics
        .iter()
        .map(|(path, _)| {
            format!(r#"<manifest:file-entry manifest:full-path="{path}" manifest:media-type="image/png"/>"#)
        })
        .collect();
    let end = manifest
        .rfind("</manifest:manifest>")
        .context("манифест направления битый")?;
    manifest.insert_str(end, &entries);

    // Пересобираем архив, остальное переносим байт в байт. Порядок важен:
    // `mimetype` обязан идти первым и несжатым — иначе это уже не .odt, и
    // LibreOffice откроет его как простой ZIP или не откроет вовсе.
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflate = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i)?;
        match f.name() {
            "content.xml" => {
                out.start_file("content.xml", deflate)?;
                out.write_all(content.as_bytes())?;
            }
            "META-INF/manifest.xml" => {
                out.start_file("META-INF/manifest.xml", deflate)?;
                out.write_all(manifest.as_bytes())?;
            }
            _ => out.raw_copy_file(f)?,
        }
    }
    for (path, png) in &pics {
        out.start_file(*path, deflate)?;
        out.write_all(png)?;
    }
    Ok(out.finish()?.into_inner())
}

/// Абзац, в который ставится подпись врача: пустая ячейка справа от строки
/// «Врач-педиатр участковый, …». Ищем её от первой подписи «Фамилия, имя,
/// отчество (при наличии), подпись» — это строка ПОД ней, — и берём последний
/// абзац предыдущей строки таблицы.
fn signature_paragraph(xml: &str) -> Option<usize> {
    let caption = xml.find("Фамилия, имя, отчество (при наличии), подпись")?;
    let row = xml[..caption].rfind("<table:table-row")?;
    let prev = xml[..row].rfind("<table:table-row")?;
    xml[prev..row].rfind("<text:p").map(|i| prev + i)
}

/// Вставить рамку сразу за открывающим тегом абзаца. Пустой абзац бывает
/// записан самозакрытым (`<text:p …/>`) — тогда раскрываем его.
fn insert_in_paragraph(xml: &str, at: usize, frame: &str) -> String {
    let close = at + xml[at..].find('>').unwrap_or(0);
    let mut out = String::with_capacity(xml.len() + frame.len() + 16);
    if xml[..close].ends_with('/') {
        out.push_str(&xml[..close - 1]);
        out.push('>');
        out.push_str(frame);
        out.push_str("</text:p>");
    } else {
        out.push_str(&xml[..=close]);
        out.push_str(frame);
    }
    out.push_str(&xml[close + 1..]);
    out
}

/// Рамка с картинкой. Высота — по пропорциям самой картинки: растянутая
/// печать выглядит подделкой.
fn ink_frame(name: &str, href: &str, png: &[u8], width_cm: f32, x: f32, y: f32) -> Result<String> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(png))
        .with_guessed_format()?
        .into_dimensions()
        .context("картинка печати или подписи не читается")?;
    let height_cm = width_cm * h as f32 / w.max(1) as f32;
    Ok(format!(
        r#"<draw:frame draw:style-name="vypiskaInk" draw:name="{name}" text:anchor-type="paragraph" svg:x="{x:.2}cm" svg:y="{y:.2}cm" svg:width="{width_cm:.2}cm" svg:height="{height_cm:.2}cm" draw:z-index="50"><draw:image xlink:href="{href}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/></draw:frame>"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn referral() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/referral.odt"
        ))
        .expect("образец направления лежит рядом с проектом")
    }

    fn dot() -> Vec<u8> {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 120, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png рисуется");
        png.into_inner()
    }

    /// Врач: «На направлении тоже печать надо, чтоб была, и подпись». Ставим
    /// в сам .odt, и после этого он обязан остаться .odt: `mimetype` первым и
    /// несжатым — иначе LibreOffice примет его за простой ZIP.
    #[test]
    fn the_referral_gets_the_stamp_and_the_signature_and_stays_an_odt() {
        let out = with_ink(&referral(), Some(&dot()), 2.9, Some(&dot())).expect("ставится");
        assert!(crate::package::is_odt(&out), "после правки это уже не .odt");

        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&out)).unwrap();
        let first = zip.by_index_raw(0).unwrap();
        assert_eq!(first.name(), "mimetype", "mimetype не первым");
        assert_eq!(
            first.compression(),
            zip::CompressionMethod::Stored,
            "mimetype сжат"
        );
        drop(first);

        let mut content = String::new();
        zip.by_name("content.xml")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(
            content.contains("Pictures/vypiska-pechat.png"),
            "печати нет в тексте"
        );
        assert!(
            content.contains("Pictures/vypiska-podpis.png"),
            "подписи нет в тексте"
        );
        assert!(
            zip.by_name("Pictures/vypiska-pechat.png").is_ok(),
            "картинки печати нет в архиве"
        );
        assert!(
            zip.by_name("Pictures/vypiska-podpis.png").is_ok(),
            "картинки подписи нет в архиве"
        );

        // И сам документ не пострадал: разбирается так же, как исходный.
        let n = parse_xml(&content).expect("после правки направление не разбирается");
        assert_eq!(n.fio, "Тестов Алексей Примерович");
    }

    /// Подпись встаёт в ячейку врача перед строкой заведующего.
    #[test]
    fn the_signature_goes_to_the_doctor_not_to_the_head_of_department() {
        let xml = {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(referral())).unwrap();
            let mut s = String::new();
            zip.by_name("content.xml")
                .unwrap()
                .read_to_string(&mut s)
                .unwrap();
            s
        };
        let at = signature_paragraph(&xml).expect("ячейка подписи нашлась");
        let doctor = xml.find("Врач-педиатр участковый").unwrap();
        let head = xml.rfind("Заведующий отделением").unwrap();
        assert!(doctor < at && at < head, "подпись встала не в строку врача");
    }

    /// Не нашёлся якорь — ошибка, а не тихо пропущенная печать.
    #[test]
    fn a_referral_without_the_stamp_line_is_refused_out_loud() {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(referral())).unwrap();
        let mut content = String::new();
        zip.by_name("content.xml")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        // Собираем копию направления, где строки «М.П.» нет.
        let broken = content.replace("М.П.", "—");
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).unwrap();
            if f.name() == "content.xml" {
                drop(f);
                out.start_file("content.xml", zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut out, broken.as_bytes()).unwrap();
            } else {
                out.raw_copy_file(f).unwrap();
            }
        }
        let odt = out.finish().unwrap().into_inner();
        let e = with_ink(&odt, Some(&dot()), 2.9, None).unwrap_err();
        assert!(format!("{e:#}").contains("печать не поставлена"), "{e:#}");
    }

    /// Если qMS сменит бланк, подписи полей разъедутся и всё разберётся
    /// пустым. Молчать об этом нельзя: врач увидит «не хватает ФИО» при
    /// загруженном файле и будет искать ошибку у себя, хотя вписать ФИО
    /// в программе негде.
    #[test]
    fn empty_parse_is_refused_out_loud() {
        let err = recognised(&Napravlenie::default()).unwrap_err().to_string();
        assert!(
            err.contains("не похоже на направление"),
            "невнятный отказ: {err}"
        );
        assert!(err.contains("бланк"), "не названа вероятная причина: {err}");
    }

    /// Обратная сторона: настоящее направление проверка пропускает молча.
    /// Достаточно ФИО — остальные поля бывают пустыми и в живых файлах.
    #[test]
    fn a_referral_with_a_name_passes() {
        let n = Napravlenie {
            fio: "Иванов Иван Иванович".into(),
            ..Default::default()
        };
        assert!(recognised(&n).is_ok());
    }
}
