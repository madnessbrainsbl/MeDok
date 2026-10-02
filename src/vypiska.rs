//! Выписка (форма 27) -> PDF. Вёрстку делает Typst по assets/forma27.typ:
//! перенос длинного анамнеза на вторую страницу нужен постоянно
//! Длинный анамнез может перейти на вторую страницу.

use anyhow::{bail, Context, Result};
use typst::foundations::{Bytes, Dict, IntoValue, Value};
use typst_as_lib::TypstEngine;

static TEMPLATE: &str = include_str!("../assets/forma27.typ");

/// Постоянный текст осмотра. Отличаются только жалобы, ЧДД и ЧСС —
/// Перед использованием текст должен быть проверен и при необходимости изменён врачом.
pub const EXAM_DEFAULT: &str = "Жалобы на момент осмотра {жалобы}. \
Состояние и самочувствие не нарушено. Сон достаточный. \
Кожные покровы бледно-розовые, умеренной влажности, чистые. \
Слизистые оболочки чистые. Склеры обычной окраски. Зев спокоен. \
Носовое дыхание свободное. Дыхание в легких везикулярное. Хрипов нет. \
ЧДД {чдд} в мин. Сердечные тоны громкие, ритмичные. ЧСС {чсс} в мин. \
Живот мягкий, безболезненный при пальпации. Печень не увеличена. \
Селезенка не пальпируется. Стул регулярный, оформленный. \
Мочеиспускание безболезненное. Менингеальные знаки отсутствуют. \
За последние 30 календарных дней за медицинской помощью по поводу \
инфекционных заболеваний не обращался.";

pub const ATTACHMENTS_DEFAULT: &str = "Результаты обследований прилагаются.";

/// Постоянный текст анамнеза жизни: заготовка, которую врач проверяет для пациента.
///
/// Правки после сверки с методичками кафедр пропедевтики (см. память
/// проекта): «Гепатит, ВИЧ, сифилис отрицает» по смыслу — это результат
/// обследования МАТЕРИ при беременности, а не самого ребёнка, и фраза без
/// подлежащего это стирала; добавлены вскармливание и аллергоанамнез —
/// стандартные пункты анамнеза жизни, которых в черновике не было. «НКПП»
/// (национальный календарь профилактических прививок) — сверенное сокращение,
/// не сокращение-догадка.
///
/// Прочерки — места, которые врач заполняет или переписывает, скопировав
/// данные из qMS. Текст обычный, не подстановочный: анамнез жизни у
/// каждого свой, и жёсткие поля тут только мешали бы.
pub const LIFE_DEFAULT: &str = "Родился от ___ беременности, ___ срочных родов в ___ недель без асфиксии, оценка по шкале Апгар ___. Вскармливание ___. Рос и развивался соответственно возрасту. Прививки по НКПП. ОРВИ ___ раза в год. Аллергоанамнез не отягощён. У матери гепатит, ВИЧ, сифилис при обследовании — отрицательны.";

/// Ширина оттиска печати по умолчанию, в сантиметрах. Крутится в настройках.
pub const STAMP_CM_DEFAULT: f32 = 2.9;

/// Разумные пределы: меньше двух сантиметров оттиск нечитаем, больше пяти —
/// он наезжает на текст и уходит за край блока подписи.
pub const STAMP_CM_RANGE: std::ops::RangeInclusive<f32> = 2.0..=5.0;

/// Подстановки, без которых текст осмотра теряет измеренные значения.
/// Названы так, чтобы из одной строки внизу окна было понятно и ЧТО не так,
/// и ГДЕ это чинится. Прежнее «место для ЧДД в тексте осмотра» называло
/// поломку, но не путь к ней: врач стёрла подстановки, правя текст осмотра, и
/// осталась с сообщением, которое не подсказывало, куда идти. Кнопка
/// «Вернуть исходный текст» там же, в настройках, и починка — одно нажатие.
const PLACEHOLDERS: [(&str, &str); 3] = [
    ("{жалобы}", "стёрто место для жалоб"),
    ("{чдд}", "стёрто место для ЧДД"),
    ("{чсс}", "стёрто место для ЧСС"),
];

/// По этому началу строки окно узнаёт поломку шаблона среди прочих нехваток
/// и дописывает к списку одну общую подсказку, куда идти чинить. Дописывать
/// путь к каждому из трёх имён нельзя: стирают их обычно все разом, и строка
/// внизу окна превратилась бы в три одинаковых абзаца.
pub const PLACEHOLDER_LOST: &str = "стёрто место";

#[derive(Debug, Default, Clone)]
pub struct Vypiska {
    pub clinic: String,
    pub fio: String,
    pub dob: String,
    pub address: String,
    pub phone: String,
    pub snils: String,
    pub birth_cert: String,
    /// Что в `birth_cert`: паспорт или свидетельство о рождении. От этого
    /// зависит подпись строки в документе — у взрослых и подростков с 14 лет
    /// документ паспорт, и строка «Свидетельство о рождении» с номером
    /// паспорта была бы неправдой.
    pub passport: bool,
    pub policy: String,
    pub organized: String,
    /// Клинический диагноз, а не код повода из направления. Решение остаётся за врачом.
    pub diagnosis: String,
    pub anamnesis_life: String,
    pub anamnesis_disease: String,

    /// Три поля, которые врач вводит на каждом пациенте.
    pub complaints: String,
    pub chdd: String,
    pub chss: String,
    /// Постоянный текст с местами под эти три. Правится в форме, хранится между запусками.
    pub exam_template: String,

    pub referral_note: String,
    pub attachments: String,
    pub date: String,
    pub doctor: String,
    /// Заведующий отделением. В выписке НЕ печатается — врач попросил строку
    /// убрать (документ уходит за её подписью). Поле оставлено намеренно:
    /// заведующий свой у каждого пациента, разбирается из настоящего
    /// направления (`odt::Napravlenie::head_of_dept`) и доезжает до шаблона,
    /// так что вернуть строку — правка только в `assets/forma27.typ`.
    pub head_of_dept: String,

    pub stamp: Option<Vec<u8>>,
    pub sign: Option<Vec<u8>>,

    /// Ширина оттиска печати в сантиметрах.
    ///
    /// Настраивается, поскольку клише у каждого врача своё.
    pub stamp_cm: f32,
}

impl Vypiska {
    pub fn new() -> Self {
        Self {
            clinic: "Тестовая детская поликлиника".into(),
            exam_template: EXAM_DEFAULT.into(),
            attachments: ATTACHMENTS_DEFAULT.into(),
            stamp_cm: STAMP_CM_DEFAULT,
            ..Default::default()
        }
    }

    /// Готовый текст осмотра: постоянная часть плюс три сегодняшних значения.
    pub fn exam(&self) -> String {
        self.exam_template
            .replace("{жалобы}", self.complaints.trim())
            .replace("{чдд}", self.chdd.trim())
            .replace("{чсс}", self.chss.trim())
    }

    /// Чего не хватает, чтобы документ можно было печатать.
    ///
    /// ЧДД и ЧСС — собственный осмотр врача, подставить их неоткуда.
    /// Отдельно проверяются подстановки в шаблоне: врач правит его руками, и
    /// стёртое `{чдд}` молча выбрасывало измеренную частоту дыхания из
    /// документа — поле при этом оставалось заполненным.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        for (val, name) in [
            (&self.fio, "ФИО пациента"),
            (&self.dob, "дата рождения"),
            (&self.diagnosis, "диагноз"),
            (&self.doctor, "врач"),
            (&self.complaints, "жалобы"),
            (&self.chdd, "ЧДД"),
            (&self.chss, "ЧСС"),
        ] {
            if val.trim().is_empty() {
                m.push(name);
            }
        }
        for (ph, name) in PLACEHOLDERS {
            if !self.exam_template.contains(ph) {
                m.push(name);
            }
        }
        m
    }

    pub fn is_ready(&self) -> bool {
        self.missing().is_empty()
    }
}

impl From<Vypiska> for Dict {
    fn from(v: Vypiska) -> Dict {
        let exam = v.exam();
        let mut d = Dict::new();
        for (k, s) in [
            ("clinic", v.clinic),
            ("fio", v.fio),
            ("dob", v.dob),
            ("address", v.address),
            ("phone", v.phone),
            ("snils", v.snils),
            ("birth_cert", v.birth_cert),
            (
                "id_doc",
                if v.passport { "Паспорт:" } else { "Свидетельство о рождении:" }.to_string(),
            ),
            ("policy", v.policy),
            ("organized", v.organized),
            ("diagnosis", v.diagnosis),
            ("anamnesis_life", v.anamnesis_life),
            ("anamnesis_disease", v.anamnesis_disease),
            ("exam", exam),
            ("referral_note", v.referral_note),
            ("attachments", v.attachments),
            ("date", v.date),
            ("doctor", v.doctor),
            ("head_of_dept", v.head_of_dept),
        ] {
            d.insert(k.into(), s.into_value());
        }
        let img = |b: Option<Vec<u8>>| match b {
            Some(bytes) => Bytes::new(bytes).into_value(),
            None => Value::None,
        };
        // Размер печати уходит числом в строке: настройки Typst приходят
        // строками, и шаблон сам переводит его в сантиметры.
        d.insert("stamp_cm".into(), format!("{:.1}", v.stamp_cm).into_value());
        d.insert("stamp".into(), img(v.stamp));
        d.insert("sign".into(), img(v.sign));
        d
    }
}

/// Libertinus Serif вшит в exe — чтобы программа оставалась переносимой
/// и набирала кириллицу на любой машине, даже без установленных шрифтов.
static FALLBACK: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/LibertinusSerif-Regular.otf"),
    include_bytes!("../assets/fonts/LibertinusSerif-Bold.otf"),
    include_bytes!("../assets/fonts/LibertinusSerif-Italic.otf"),
    include_bytes!("../assets/fonts/LibertinusSerif-BoldItalic.otf"),
];

/// Имена файлов Times New Roman в системной папке шрифтов: обычный,
/// полужирный, курсив, полужирный курсив.
#[cfg(windows)]
pub const TIMES_FILES: [&str; 4] = ["times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf"];

/// На Linux настоящего Times New Roman в системе нет и вшить его нельзя —
/// шрифт проприетарный (Monotype). Но он появляется, если администратор
/// поставит пакет с MS-шрифтами из репозитория БАЗАЛЬТа (имена в ветках
/// разные — ищется `apt-cache search ttf-ms`), и тогда бланк надо набирать
/// именно им: врач просил Times, а не «что-нибудь похожее». Раскладка по
/// файлам у разных сборок своя — msttcorefonts кладёт `times.ttf`, сборки
/// БАЗАЛЬТа — `Times_New_Roman.ttf`, отсюда несколько наборов.
///
/// Последний набор — метрический близнец Liberation Serif из
/// `fonts-ttf-liberation`: у него те же ширины символов, что у Times, и на
/// «Альте» он есть всегда. Берём его только если настоящего не нашлось.
#[cfg(unix)]
const UNIX_SERIF_SETS: [[&str; 4]; 4] = [
    ["times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf"],
    [
        "Times_New_Roman.ttf",
        "Times_New_Roman_Bold.ttf",
        "Times_New_Roman_Italic.ttf",
        "Times_New_Roman_Bold_Italic.ttf",
    ],
    [
        "timesnewroman.ttf",
        "timesnewromanbold.ttf",
        "timesnewromanitalic.ttf",
        "timesnewromanbolditalic.ttf",
    ],
    [
        "LiberationSerif-Regular.ttf",
        "LiberationSerif-Bold.ttf",
        "LiberationSerif-Italic.ttf",
        "LiberationSerif-BoldItalic.ttf",
    ],
];

/// Первый набор, который нашёлся в системе. Считается один раз: обход папок
/// шрифтов и без того кэширован, а сам выбор попадает в журнал полным путём —
/// по нему сразу видно, настоящий Times взяли или замену.
#[cfg(unix)]
pub static TIMES_FILES: std::sync::LazyLock<[&'static str; 4]> = std::sync::LazyLock::new(|| {
    UNIX_SERIF_SETS
        .iter()
        .find(|set| crate::find_font(set[0]).is_some())
        .copied()
        .unwrap_or(UNIX_SERIF_SETS[UNIX_SERIF_SETS.len() - 1])
});

/// Отдаём движку оба набора: системный Times (или его близнец), если он
/// есть, и вшитый Libertinus. Какой применить — решает список шрифтов в
/// шаблоне, и имена в нём должны совпадать с именами семейств, которые
/// приезжают сюда: Liberation Serif в шаблоне раньше не значился, поэтому на
/// Linux бланк молча набирался вшитым Libertinus.
fn fonts() -> Vec<Vec<u8>> {
    let mut fonts: Vec<Vec<u8>> = TIMES_FILES
        .iter()
        .filter_map(|n| crate::find_font(n))
        .filter_map(|p| std::fs::read(p).ok())
        .collect();
    fonts.extend(FALLBACK.iter().map(|b| b.to_vec()));
    fonts
}

fn compile(v: Vypiska) -> Result<typst_layout::PagedDocument> {
    let missing = v.missing();
    if !missing.is_empty() {
        bail!("не заполнено: {}", missing.join(", "));
    }
    let engine = TypstEngine::builder()
        .main_file(TEMPLATE)
        .fonts(fonts())
        .build();
    engine
        .compile_with_input::<Vypiska, typst_layout::PagedDocument>(v)
        .output
        .map_err(|e| anyhow::anyhow!("Typst не собрал выписку: {e:?}"))
}

pub fn to_pdf(v: Vypiska) -> Result<Vec<u8>> {
    let doc = compile(v)?;
    typst_pdf::pdf(&doc, &Default::default())
        .map_err(|e| anyhow::anyhow!("не собрался PDF: {e:?}"))
        .context("вёрстка выписки")
}

/// Страницы выписки картинками.
///
/// Нужны для пакета: выписка — векторный PDF, и вытащить из неё «снимок
/// страницы», как из скана, нельзя. Попытка это сделать давала в пакете
/// печать, растянутую на весь лист, потому что из вложенных картинок она
/// оказывалась самой крупной. Поэтому рисуем страницы сами.
pub fn to_images(v: Vypiska, width_px: u32) -> Result<Vec<image::RgbImage>> {
    let doc = compile(v)?;
    let mut out = Vec::new();
    for page in doc.pages() {
        let pt_w = page.frame.width().to_pt().max(1.0);
        let scale = width_px as f64 / pt_w;
        let opts = typst_render::RenderOptions {
            pixel_per_pt: typst_utils::Scalar::new(scale),
            ..Default::default()
        };
        let pix = typst_render::render(page, &opts);
        let (w, h) = (pix.width(), pix.height());
        // Typst отдаёт premultiplied RGBA; лист белый, поэтому кладём на белое.
        let rgb: Vec<u8> = pix
            .pixels()
            .iter()
            .flat_map(|p| {
                let a = p.alpha() as u32;
                let over = |c: u8| (((c as u32) + 255 * (255 - a) / 255).min(255)) as u8;
                [over(p.red()), over(p.green()), over(p.blue())]
            })
            .collect();
        let img = image::RgbImage::from_raw(w, h, rgb)
            .context("не сложить страницу выписки в картинку")?;
        out.push(img);
    }
    if out.is_empty() {
        bail!("выписка вышла пустой");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Весь текст документа одной строкой. PDF сравнивать на подстроку нельзя —
    /// Typst сжимает содержимое и кодирует текст глифами, а не читаемыми байтами —
    /// поэтому проверка идёт по скомпилированному `Frame`, откуда PDF ещё не собран.
    fn all_text(doc: &typst_layout::PagedDocument) -> String {
        fn walk(frame: &typst::layout::Frame, out: &mut String) {
            for (_, item) in frame.items() {
                match item {
                    typst::layout::FrameItem::Text(t) => out.push_str(&t.text),
                    typst::layout::FrameItem::Group(g) => walk(&g.frame, out),
                    _ => {}
                }
            }
        }
        let mut out = String::new();
        for page in doc.pages() {
            walk(&page.frame, &mut out);
        }
        out
    }

    fn filled() -> Vypiska {
        let mut v = Vypiska::new();
        v.fio = "Тестов Алексей Примерович".into();
        v.dob = "5 мая 2016 года (10 лет)".into();
        v.diagnosis = "Z00.1 Профилактический осмотр".into();
        v.doctor = "Примерова П. П.".into();
        v.chdd = "19".into();
        v.chss = "82".into();
        v.complaints = "активно не предъявляет".into();
        v
    }

    /// Размер печати задаёт врач, и число из настроек уходит в шаблон
    /// строкой, а сантиметры из неё делает уже Typst. Если это место
    /// сломать, документ перестанет собираться ровно тогда, когда печать
    /// загружена, — то есть у врача на приёме, а не здесь.
    #[test]
    fn the_stamp_size_from_the_settings_reaches_the_document() {
        // Одноточечный PNG: содержимое неважно, важно, что печать есть и
        // ветка с её размером в шаблоне выполняется. Рисуем его крейтом
        // `image`, а не байтами из головы: у собранного вручную PNG не
        // сходится контрольная сумма, и тест падал бы на своём же образце.
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 90, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png рисуется");
        let dot = png.into_inner();
        for cm in [
            *STAMP_CM_RANGE.start(),
            STAMP_CM_DEFAULT,
            *STAMP_CM_RANGE.end(),
        ] {
            let mut v = filled();
            v.stamp = Some(dot.clone());
            v.stamp_cm = cm;
            if let Err(e) = to_pdf(v) {
                panic!("документ не собрался с размером печати {cm} см: {e:#}");
            }
        }
    }

    /// У взрослых и подростков с 14 лет документ — паспорт. Строка
    /// «Свидетельство о рождении» с номером паспорта была бы неправдой в
    /// документе, который уходит в другое учреждение.
    #[test]
    fn the_document_line_says_what_the_document_is() {
        let mut v = filled();
        v.birth_cert = "0412 345678".into();
        v.passport = true;
        let text = all_text(&compile(v).expect("собирается"));
        assert!(text.contains("Паспорт"), "в документе нет строки «Паспорт»");
        assert!(!text.contains("Свидетельство о рождении"), "осталась строка свидетельства");

        let mut v = filled();
        v.birth_cert = "III-АА 000001".into();
        let text = all_text(&compile(v).expect("собирается"));
        assert!(text.contains("Свидетельство о рождении"), "у ребёнка пропало свидетельство");
    }

    #[test]
    fn vitals_are_required_because_they_are_her_own_examination() {
        let mut v = filled();
        v.chdd.clear();
        assert!(!v.is_ready());
        assert!(v.missing().contains(&"ЧДД"));
        v.chdd = "19".into();
        assert!(v.is_ready(), "не хватает: {:?}", v.missing());
    }

    #[test]
    fn doctor_and_complaints_are_required_too() {
        let mut v = filled();
        v.doctor.clear();
        assert!(v.missing().contains(&"врач"));
        let mut v = filled();
        v.complaints.clear();
        assert!(v.missing().contains(&"жалобы"));
    }

    /// Врач правит постоянный текст руками. Стёртое `{чдд}` раньше просто
    /// выбрасывало измеренную частоту дыхания, и документ считался готовым.
    #[test]
    fn erased_placeholder_blocks_the_document() {
        let mut v = filled();
        v.exam_template = "Жалобы на момент осмотра {жалобы}. ЧСС {чсс} в мин.".into();
        assert!(!v.is_ready(), "документ без ЧДД не должен собираться");
        // Сообщение обязано вести к месту починки, а не только называть
        // поломку: врач стёрла подстановки, правя текст осмотра, и осталась
        // с сообщением, из которого не следовало, куда идти.
        assert!(v.missing().contains(&"стёрто место для ЧДД"));
        assert!(to_pdf(v).is_err());
    }

    #[test]
    fn exam_text_takes_the_three_values() {
        let v = filled();
        let e = v.exam();
        assert!(e.contains("ЧДД 19 в мин"), "{e}");
        assert!(e.contains("ЧСС 82 в мин"), "{e}");
        assert!(
            e.contains("Жалобы на момент осмотра активно не предъявляет."),
            "{e}"
        );
        assert!(!e.contains('{'), "в тексте осталась подстановка: {e}");
    }

    /// Шаблон Typst раньше не компилировался ни одним тестом — ошибка в вёрстке
    /// обнаруживалась только глазами на готовом PDF.
    #[test]
    fn template_compiles_and_contains_the_patient() {
        let mut v = filled();
        v.address = "Россия, ул Примерная, д.1".into();
        v.anamnesis_disease = "Тестовый анамнез заболевания.".into();
        let pdf = to_pdf(v).expect("шаблон должен компилироваться");
        assert!(pdf.starts_with(b"%PDF"), "на выходе не PDF");
        assert!(
            pdf.len() > 20_000,
            "подозрительно маленький PDF: {}",
            pdf.len()
        );
    }

    /// Заведующей в выписке быть не должно вовсе — даже когда она известна и
    /// доехала до шаблона. Проверяем именно заполненным полем: пустое ничего
    /// не доказывает, оно не печаталось и раньше.
    #[test]
    fn head_of_dept_never_reaches_the_document() {
        let mut v = filled();
        v.address = "Россия, ул Примерная, д.1".into();
        v.anamnesis_disease = "Тестовый анамнез заболевания.".into();
        v.head_of_dept = "Старшая С. С.".into();
        let doc = compile(v).expect("шаблон должен компилироваться");
        let text = all_text(&doc);
        assert!(
            !text.contains("Заведующий отделением"),
            "строка заведующей вернулась в выписку"
        );
        assert!(
            !text.contains("Старшая С. С."),
            "фамилия заведующей попала в документ"
        );
    }

    /// Семейства шрифтов, которыми реально набран документ.
    fn fonts_used(doc: &typst_layout::PagedDocument) -> Vec<String> {
        fn walk(frame: &typst::layout::Frame, out: &mut Vec<String>) {
            for (_, item) in frame.items() {
                match item {
                    typst::layout::FrameItem::Text(t) => {
                        let f = t.font.info().family.clone();
                        if !out.contains(&f) {
                            out.push(f);
                        }
                    }
                    typst::layout::FrameItem::Group(g) => walk(&g.frame, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for page in doc.pages() {
            walk(&page.frame, &mut out);
        }
        out
    }

    /// Бланк набирается Times New Roman или его метрическим близнецом
    /// Liberation Serif, но не вшитым Libertinus: у него другой рисунок, и
    /// врач это на бумаге увидел. Ровно эта ошибка и была на Linux —
    /// Liberation отдавали движку, а в списке шрифтов шаблона его имени не
    /// было, и Typst до него не доходил.
    #[test]
    fn blank_is_set_in_times_or_its_metric_twin() {
        if crate::find_font(TIMES_FILES[0]).is_none() {
            // Системного шрифта с засечками нет вовсе — набирать нечем, кроме
            // вшитого. Это законный запасной путь, а не ошибка вёрстки.
            return;
        }
        let mut v = filled();
        v.address = "Россия, ул Примерная, д.1".into();
        let doc = compile(v).expect("шаблон должен компилироваться");
        let used = fonts_used(&doc);
        assert!(
            used.iter()
                .any(|f| f == "Times New Roman" || f == "Liberation Serif"),
            "бланк набран не Times и не его близнецом: {used:?}"
        );
        assert!(
            !used.iter().any(|f| f.starts_with("Libertinus")),
            "в бланк пролез вшитый запасной шрифт: {used:?}"
        );
    }
}
