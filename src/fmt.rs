//! Даты и возраст в том виде, в каком они стоят в бланках.
//!
//! Возраст вычисляется из даты направления, чтобы избежать расхождения при ручном вводе.
//!
//! Всё, что не удалось разобрать уверенно, отдаётся пустой строкой, а не
//! правдоподобной чепухой: пустое поле поймает `missing()`, а «9 мес» у
//! девятилетнего не поймает никто.

use crate::odt::{Dob, MONTHS};
use chrono::{Datelike, NaiveDate};

/// Раньше этого года дат рождения у детской поликлиники не бывает —
/// всё, что меньше, это не разобранный год, а не столетний пациент.
const EARLIEST_YEAR: i32 = 1900;

/// Месяц по названию: «октября» -> 10. Ноль, если не узнали.
pub fn month_num(name: &str) -> u32 {
    let n = name.trim().to_lowercase();
    MONTHS
        .iter()
        .position(|m| *m == n)
        .map(|i| i as u32 + 1)
        .unwrap_or(0)
}

pub fn to_date(d: &Dob) -> Option<NaiveDate> {
    if d.year < EARLIEST_YEAR {
        return None;
    }
    NaiveDate::from_ymd_opt(d.year, month_num(&d.month), d.day)
}

/// Время для журнала запуска: «14:07:31». Дата там не нужна — журнал
/// перезаписывается при каждом запуске и описывает один сеанс.
pub fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// Полных лет и месяцев на дату приёма. `None`, если дата рождения в будущем.
fn years_months(dob: NaiveDate, on: NaiveDate) -> Option<(i32, i32)> {
    if on < dob {
        return None;
    }
    let mut years = on.year() - dob.year();
    let mut months = on.month() as i32 - dob.month() as i32;
    if on.day() < dob.day() {
        months -= 1;
    }
    if months < 0 {
        years -= 1;
        months += 12;
    }
    Some((years, months))
}

/// Возраст в месяцах на дату приёма — вход для проверки норм ЧДД/ЧСС
/// (`norms.rs`). `None` на тех же условиях, что и у `age_str`: дата рождения
/// не разобралась или лежит в будущем.
pub fn age_months(dob: NaiveDate, on: NaiveDate) -> Option<i32> {
    years_months(dob, on).map(|(y, m)| y * 12 + m)
}

/// «12 лет», «9 мес», «1 год». До года считаем месяцами — так в её выписках.
/// Пустая строка, если дата рождения позже даты приёма.
pub fn age_str(dob: NaiveDate, on: NaiveDate) -> String {
    let Some((y, m)) = years_months(dob, on) else {
        return String::new();
    };
    if y < 1 {
        return format!("{m} мес");
    }
    format!("{y} {}", plural_year(y))
}

fn plural_year(y: i32) -> &'static str {
    let (a, b) = (y % 10, y % 100);
    if (11..=14).contains(&b) {
        "лет"
    } else if a == 1 {
        "год"
    } else if (2..=4).contains(&a) {
        "года"
    } else {
        "лет"
    }
}

/// Строка для выписки: «12 июля 2014 года (11 лет)».
/// Пустая, если дату не удалось разобрать или она в будущем — пустое поле
/// остановит сборку документа, а выдуманный возраст прошёл бы незамеченным.
pub fn dob_full(d: &Dob, on: NaiveDate) -> String {
    let Some(date) = to_date(d) else {
        return String::new();
    };
    let age = age_str(date, on);
    if age.is_empty() {
        return String::new();
    }
    format!("{} {} {} года ({})", d.day, d.month, d.year, age)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dob(day: u32, month: &str, year: i32) -> Dob {
        Dob {
            day,
            month: month.to_string(),
            year,
        }
    }
    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn age_matches_a_sample_referral() {
        let got = dob_full(&dob(12, "июля", 2014), d(2026, 3, 17));
        assert_eq!(got, "12 июля 2014 года (11 лет)");
    }

    #[test]
    fn infants_are_counted_in_months() {
        let got = dob_full(&dob(10, "января", 2025), d(2025, 10, 1));
        assert_eq!(got, "10 января 2025 года (8 мес)");
        let got = dob_full(&dob(7, "марта", 2025), d(2025, 12, 15));
        assert_eq!(got, "7 марта 2025 года (9 мес)");
    }

    /// Синтетические даты для проверки расчёта возраста в месяцах.
    #[test]
    fn age_months_matches_sample_dates() {
        assert_eq!(age_months(d(2014, 7, 12), d(2026, 3, 17)), Some(140));
        assert_eq!(age_months(d(2025, 1, 10), d(2025, 10, 1)), Some(8));
        assert_eq!(
            age_months(d(2026, 9, 16), d(2026, 9, 15)),
            None,
            "рождение в будущем"
        );
    }

    #[test]
    fn birthday_not_yet_reached_does_not_round_up() {
        let born = d(2014, 7, 12);
        assert_eq!(age_str(born, d(2026, 7, 11)), "11 лет");
        assert_eq!(age_str(born, d(2026, 7, 12)), "12 лет");
    }

    #[test]
    fn russian_plural_for_years() {
        let born = d(2020, 1, 1);
        assert_eq!(age_str(born, d(2021, 1, 1)), "1 год");
        assert_eq!(age_str(born, d(2023, 1, 1)), "3 года");
        assert_eq!(age_str(born, d(2025, 1, 1)), "5 лет");
        assert_eq!(age_str(born, d(2031, 1, 1)), "11 лет");
        assert_eq!(age_str(born, d(2041, 1, 1)), "21 год");
    }

    /// Опечатка в годе направления давала «9 мес» у школьника — возраст
    /// считался по дате рождения из будущего.
    #[test]
    fn birth_date_in_the_future_yields_nothing() {
        let on = d(2026, 9, 15);
        assert_eq!(age_str(d(2026, 9, 16), on), "");
        assert_eq!(dob_full(&dob(16, "сентября", 2026), on), "");
        assert_eq!(dob_full(&dob(1, "января", 2030), on), "");
    }

    /// Неразобранный год приходил нулём, а год 0 — валидная дата,
    /// поэтому выписка печаталась с «17 октября 0 года (2025 лет)».
    #[test]
    fn unparsed_year_yields_nothing() {
        let on = d(2026, 9, 15);
        assert_eq!(dob_full(&dob(17, "октября", 0), on), "");
        assert_eq!(dob_full(&dob(17, "октября", 13), on), "");
        assert!(to_date(&dob(17, "октября", 1899)).is_none());
    }

    #[test]
    fn unknown_month_or_day_yields_nothing() {
        let on = d(2026, 9, 15);
        assert_eq!(dob_full(&dob(17, "", 2013), on), "");
        assert_eq!(dob_full(&dob(0, "октября", 2013), on), "");
        assert_eq!(dob_full(&dob(31, "февраля", 2013), on), "");
    }
}
