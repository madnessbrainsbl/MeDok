//! Возрастные нормы ЧДД и ЧСС — тихая подсказка, а не запрет.
//!
//! Она предупреждает: «для 9 мес ЧСС 82 ниже нормы, проверьте» — и не мешает
//! сохранить документ. ЧДД и ЧСС — это её собственный осмотр (см. правило в
//! памяти проекта), подставлять их нельзя; здесь только сверка того, что она
//! уже вписала, с диапазоном.
//!
//! Границы — 1-й и 99-й перцентиль по Fleming S. et al., Lancet 2011;
//! 377(9770):1011-1018 (DOI 10.1016/S0140-6736(10)62226-X): систематический
//! обзор, ЧСС — 143 346 детей, ЧДД — 3 881 ребёнок. Это самый цитируемый в
//! мире источник по нормам витальных показателей у детей и единственный из
//! проверенных, где границы — не точка, а диапазон: точечное «в 12 лет ЧСС
//! 80» (Мазурин, Воронцов, 1987) ложно пометило бы отклонением 82 или 90.
//!
//! Сверено на трёх настоящих записях практикующего педиатра: 12 лет 19/82,
//! 2 года 27/127, 9 мес 25/118 — все три укладываются в диапазон, тесты ниже
//! это закрепляют.
//!
//! ЧДД у Fleming для 8+ лет считана по вторичному пересказу (exahealth.com со
//! ссылкой на Fleming + PALS) — у самого источника ниже статистическая мощность
//! выборки ЧДД для старших детей. За верхней границей таблицы (после 15–18 лет)
//! подсказка молчит, а не гадает: у Fleming для этого возраста данных нет.

struct Band {
    /// Месяцы от рождения: `from` входит, `to` — нет.
    from: i32,
    to: i32,
    lo: u32,
    hi: u32,
}

const CHSS: &[Band] = &[
    Band {
        from: 0,
        to: 3,
        lo: 107,
        hi: 181,
    },
    Band {
        from: 3,
        to: 6,
        lo: 104,
        hi: 175,
    },
    Band {
        from: 6,
        to: 9,
        lo: 98,
        hi: 168,
    },
    Band {
        from: 9,
        to: 12,
        lo: 93,
        hi: 161,
    },
    Band {
        from: 12,
        to: 18,
        lo: 88,
        hi: 156,
    },
    Band {
        from: 18,
        to: 24,
        lo: 82,
        hi: 149,
    },
    Band {
        from: 24,
        to: 36,
        lo: 76,
        hi: 142,
    },
    Band {
        from: 36,
        to: 48,
        lo: 70,
        hi: 136,
    },
    Band {
        from: 48,
        to: 72,
        lo: 65,
        hi: 131,
    },
    Band {
        from: 72,
        to: 96,
        lo: 59,
        hi: 123,
    },
    Band {
        from: 96,
        to: 144,
        lo: 52,
        hi: 115,
    },
    Band {
        from: 144,
        to: 180,
        lo: 47,
        hi: 108,
    },
    Band {
        from: 180,
        to: 216,
        lo: 43,
        hi: 104,
    },
];

const CHDD: &[Band] = &[
    Band {
        from: 0,
        to: 3,
        lo: 25,
        hi: 66,
    },
    Band {
        from: 3,
        to: 6,
        lo: 24,
        hi: 64,
    },
    Band {
        from: 6,
        to: 9,
        lo: 23,
        hi: 61,
    },
    Band {
        from: 9,
        to: 12,
        lo: 22,
        hi: 58,
    },
    Band {
        from: 12,
        to: 18,
        lo: 21,
        hi: 53,
    },
    Band {
        from: 18,
        to: 24,
        lo: 19,
        hi: 46,
    },
    Band {
        from: 24,
        to: 36,
        lo: 18,
        hi: 38,
    },
    Band {
        from: 36,
        to: 48,
        lo: 17,
        hi: 33,
    },
    Band {
        from: 48,
        to: 72,
        lo: 17,
        hi: 29,
    },
    Band {
        from: 72,
        to: 96,
        lo: 16,
        hi: 27,
    },
    // 8–12 и 12–18 лет: вторичная оценка, точность ниже остальной таблицы.
    Band {
        from: 96,
        to: 144,
        lo: 16,
        hi: 24,
    },
    Band {
        from: 144,
        to: 216,
        lo: 12,
        hi: 20,
    },
];

fn find(bands: &[Band], age_months: i32) -> Option<&Band> {
    bands
        .iter()
        .find(|b| age_months >= b.from && age_months < b.to)
}

/// `Some((низ, верх))`, если значение вышло за 1–99 перцентиль для возраста.
/// `None` — либо в норме, либо возраст за пределами того, что измерил Fleming.
pub fn suspicious_chdd(age_months: i32, value: u32) -> Option<(u32, u32)> {
    find(CHDD, age_months)
        .filter(|b| value < b.lo || value > b.hi)
        .map(|b| (b.lo, b.hi))
}

pub fn suspicious_chss(age_months: i32, value: u32) -> Option<(u32, u32)> {
    find(CHSS, age_months)
        .filter(|b| value < b.lo || value > b.hi)
        .map(|b| (b.lo, b.hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Три настоящие записи участкового педиатра — ни одна не должна
    /// подсвечиваться отклонением. Подсказка, которая кричит на норму,
    /// хуже отсутствия подсказки: её быстро перестанут читать.
    #[test]
    fn real_records_are_not_flagged() {
        assert_eq!(suspicious_chdd(144, 19), None, "12 лет, ЧДД 19");
        assert_eq!(suspicious_chss(144, 82), None, "12 лет, ЧСС 82");
        assert_eq!(suspicious_chdd(24, 27), None, "2 года, ЧДД 27");
        assert_eq!(suspicious_chss(24, 127), None, "2 года, ЧСС 127");
        assert_eq!(suspicious_chdd(9, 25), None, "9 мес, ЧДД 25");
        assert_eq!(suspicious_chss(9, 118), None, "9 мес, ЧСС 118");
    }

    #[test]
    fn out_of_range_is_flagged_with_the_band() {
        // 9 мес, ЧСС 60 — заметно ниже 93–161.
        assert_eq!(suspicious_chss(9, 60), Some((93, 161)));
        // 2 года, ЧДД 60 — заметно выше 18–38.
        assert_eq!(suspicious_chdd(24, 60), Some((18, 38)));
    }

    #[test]
    fn band_edges_are_inclusive() {
        assert_eq!(
            suspicious_chss(9, 93),
            None,
            "нижняя граница входит в норму"
        );
        assert_eq!(
            suspicious_chss(9, 161),
            None,
            "верхняя граница входит в норму"
        );
        assert!(suspicious_chss(9, 92).is_some());
        assert!(suspicious_chss(9, 162).is_some());
    }

    /// За пределами таблицы (взрослые значения, ошибочно большой возраст)
    /// подсказка молчит — у источника для этого возраста данных нет, и
    /// гадать здесь опаснее, чем промолчать.
    #[test]
    fn beyond_the_table_is_silent_not_guessed() {
        assert_eq!(suspicious_chss(300, 70), None);
        assert_eq!(suspicious_chdd(300, 16), None);
    }
}
