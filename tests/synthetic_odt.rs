//! Проверка на синтетическом ODT той же структуры.
//! Файл в tests/fixtures/referral.odt

use std::path::PathBuf;
use vypiska::fmt;
use vypiska::odt;

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/referral.odt")
}

fn parse() -> odt::Napravlenie {
    odt::parse_file(&sample()).expect("не разобрали образец направления из КУМки")
}

#[test]
fn passport_fields_come_out_of_the_spans() {
    let n = parse();
    assert_eq!(n.fio, "Тестов Алексей Примерович");
    assert_eq!(n.policy, "0000000000000000");
    assert_eq!(n.policy_date, "01.01.2020");
    assert_eq!(n.date, "01.02.2026");
    assert_eq!(n.sex, "муж - 1");
    assert!(
        n.addr_reg.contains("Примерная, д.1"),
        "адрес: {}",
        n.addr_reg
    );
    assert!(
        n.insurer.contains("Тестовая страховая"),
        "СМО: {}",
        n.insurer
    );
}

#[test]
fn date_of_birth_comes_from_three_spans() {
    // Дата рождения состоит из трёх отдельных спанов.
    let n = parse();
    assert_eq!(n.dob.day, 5);
    assert_eq!(n.dob.month, "мая");
    assert_eq!(n.dob.year, 2016);

    let on = chrono::NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();
    assert_eq!(fmt::dob_full(&n.dob, on), "5 мая 2016 года (10 лет)");
}

#[test]
fn diagnosis_is_taken_whole() {
    let n = parse();
    assert!(n.diagnosis.starts_with("Z13.8"), "диагноз: {}", n.diagnosis);
    assert!(
        n.diagnosis.contains("скрининговое обследование"),
        "диагноз: {}",
        n.diagnosis
    );
}

#[test]
fn doctor_is_anchored_to_the_label_not_to_the_word_vrach() {
    let n = parse();
    assert_eq!(n.doctor, "Примерова П. П.");
    assert_eq!(n.doctor_post, "Врач-педиатр участковый");
    // Заведующий меняется между направлениями — поэтому он поле, а не константа.
    assert_eq!(n.head_of_dept, "Руководитель Р. Р.");
}

/// Раньше врач искался по первому абзацу со словом «Врач» во всём документе,
/// и его мог украсть любой более ранний абзац.
#[test]
fn earlier_paragraph_with_vrach_does_not_steal_the_field() {
    let xml = r#"<?xml version="1.0"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <text:p>Врач приёмного отделения <text:span>Посторонний П. П.</text:span></text:p>
  <text:p>Должность, специальность медицинского работника, направившего пациента</text:p>
  <text:p>Врач-педиатр участковый, <text:span>Примерова П. П.</text:span></text:p>
</office:document-content>"#;
    let n = odt::parse_xml(xml).unwrap();
    assert_eq!(n.doctor, "Примерова П. П.");
    assert_eq!(n.doctor_post, "Врач-педиатр участковый");
}

#[test]
fn target_organisation() {
    let n = parse();
    assert!(
        n.target_mo.starts_with("Тестовый медицинский центр"),
        "куда: {}",
        n.target_mo
    );
}

/// Неразобранный год не должен превращаться в год 0: `NaiveDate` считает его
/// валидным, и в выписку уходило «17 октября 0 года (2025 лет)».
#[test]
fn unparsable_year_does_not_become_year_zero() {
    let xml = r#"<?xml version="1.0"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <text:p>Дата рождения: «<text:span>17</text:span>» <text:span>октября</text:span> <text:span>13</text:span>г.</text:p>
</office:document-content>"#;
    let n = odt::parse_xml(xml).unwrap();
    assert_eq!(n.dob.year, 0, "год вне диапазона должен остаться нулём");
    let on = chrono::NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();
    assert_eq!(fmt::dob_full(&n.dob, on), "", "такую дату печатать нельзя");
}

/// Неразрывные пробелы и повторы схлопываются, иначе подписи полей не совпадут.
#[test]
fn nbsp_and_repeats_are_normalised() {
    let xml = "<?xml version=\"1.0\"?>\n<office:document-content \
  xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
  xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\">\
  <text:p>Фамилия, имя, отчество (при наличии) пациента\u{a0}\u{a0}<text:span>Иванов\u{a0}Иван</text:span></text:p>\
</office:document-content>";
    let n = odt::parse_xml(xml).unwrap();
    assert_eq!(n.fio, "Иванов Иван");
}

/// Каждая регистрация обязана прийти из своей строки. В этом направлении обе
/// совпадают ДО БУКВЫ, поэтому подмена одной на другую тут ничем себя не
/// выдаёт — образец её не ловит. Ловят два синтетических разбора ниже,
/// где адреса разные.
#[test]
fn both_registration_addresses_come_from_their_own_rows() {
    let n = parse();
    let addr = "г Тестоград, ул Примерная, д.1";
    assert_eq!(n.addr_reg, addr, "по месту жительства");
    assert_eq!(n.addr_stay, addr, "по месту пребывания");
    // Выше в том же документе стоит адрес самой поликлиники — он не должен
    // оказаться адресом пациента.
    assert!(
        !n.addr_reg.contains("Лечебная"),
        "в адрес пациента попал адрес поликлиники: {}",
        n.addr_reg
    );
}

/// Когда адреса РАЗНЫЕ, каждый обязан взять свою строку — в любом порядке,
/// в каком их выложит КУМка. Это то самое место, где ошибка дошла бы до
/// подписанного документа незамеченной.
#[test]
fn registration_addresses_do_not_swap() {
    let doc = |rows: &str| {
        format!(
            "<?xml version=\"1.0\"?>\n<office:document-content \
  xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
  xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\">{rows}</office:document-content>"
        )
    };
    let zhit = "<text:p>Регистрация по месту жительства: <text:span>край Тестовый, г Тестоград, ул Примерная, д.1</text:span></text:p>";
    let preb = "<text:p>Регистрация по месту пребывания: <text:span>край Тестовый, г Другоград, ул Мира, д.1, кв.2</text:span></text:p>";

    for rows in [format!("{zhit}{preb}"), format!("{preb}{zhit}")] {
        let n = odt::parse_xml(&doc(&rows)).unwrap();
        assert_eq!(
            n.addr_reg, "край Тестовый, г Тестоград, ул Примерная, д.1",
            "по месту жительства взят не свой адрес"
        );
        assert_eq!(
            n.addr_stay, "край Тестовый, г Другоград, ул Мира, д.1, кв.2",
            "по месту пребывания взят не свой адрес"
        );
    }
}

/// В «Домашний адрес» выписки идёт место пребывания, когда оно заполнено.
///
/// Почему так: строку «регистрация по месту пребывания» новая форма 057/у
/// (приказ Минздрава России от 02.09.2025 № 519н) завела именно под
/// фактическое жительство — «для пациентов это место фактического
/// жительства». Пусто — значит живёт по прописке. Подробнее и с оговорками:
/// `Napravlenie::address_by_fact`.
///
/// Тест нужен как раз потому, что образец этого не проверяет: в нём
/// обе строки совпадают, и любая из веток даст верный ответ.
#[test]
fn home_address_prefers_the_place_of_stay() {
    let mut n = odt::Napravlenie {
        addr_reg: "г Тестоград, ул Примерная, д.1".into(),
        addr_stay: "г Другоград, ул Мира, д.1, кв.2".into(),
        ..Default::default()
    };
    assert_eq!(
        n.address_by_fact(),
        "г Другоград, ул Мира, д.1, кв.2",
        "временная регистрация заполнена — она и есть фактический адрес"
    );

    // Пробелы — это тоже пусто: КУМка отдаёт строку с одним пробелом, если
    // графа в направлении не заполнена, и адрес не должен стать пустым.
    for empty in ["", " ", "\u{a0}"] {
        n.addr_stay = empty.into();
        assert_eq!(
            n.address_by_fact(),
            "г Тестоград, ул Примерная, д.1",
            "пребывание пустое ({empty:?}) — остаётся прописка"
        );
    }

    // Синтетический файл: обе строки совпадают, ответ обязан быть той же строкой.
    let sample = parse();
    assert_eq!(sample.address_by_fact(), sample.addr_reg);
    assert!(
        sample.address_by_fact().contains("Примерная, д.1"),
        "адрес в выписку: {}",
        sample.address_by_fact()
    );
}

/// КУМка рвёт одно значение на несколько спанов, как только внутри меняется
/// начертание — в строке с датой рождения таких спанов три. Пока бралcя
/// только первый спан, длинный адрес молча обрезался, а обрезанный адрес в
/// выписке выглядит как настоящий: «…г Тестоград» вместо дома и квартиры.
#[test]
fn address_split_across_spans_comes_out_whole() {
    let xml = "<?xml version=\"1.0\"?>\n<office:document-content \
  xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
  xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\">\
  <text:p>Регистрация по месту жительства: <text:span>Россия, 000000, край Тестовый, г Тестоград</text:span>, \
<text:span>ул Примерная, д.1</text:span></text:p>\
</office:document-content>";
    let n = odt::parse_xml(xml).unwrap();
    assert_eq!(
        n.addr_reg,
        "Россия, 000000, край Тестовый, г Тестоград, ул Примерная, д.1"
    );
}

/// Графа с вариантами: значение — только выбранный спан, подписи вариантов
/// после него в поле не лезут. Проверка не про адрес, а про то, что разбор
/// диапазона спанов не «съел» хвост строки.
#[test]
fn option_row_still_takes_only_the_chosen_value() {
    let n = parse();
    assert_eq!(n.sex, "муж - 1");
}
