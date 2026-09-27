//! Вырезание чернил из загруженных изображений.
//!
//! Синтетические образцы в `tests/fixtures` покрывают разные форматы:
//! * `stamp-sample.jpg` — клише печати, чёрное по белому, альфы нет вовсе;
//! * `sign-sample.png` — RGBA с непрозрачным фоном;
//! * `scan.pdf` — PDF со снимком страницы.
//!
//! Поэтому «выберите PNG с прозрачностью» не работает: такой файл никто не сделает.
//! Программа берёт что дали и вырезает сама.

use anyhow::{bail, Context, Result};
use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};

/// Ниже этого значения альфы — фактура бумаги и JPEG-звон, а не чернила.
pub const THRESHOLD_DEFAULT: u8 = 40;
/// Рабочий размер: на странице шириной 1240 px печать 40 мм занимает ~236 px.
pub const TARGET_WIDTH: u32 = 400;
/// Потолок разрешения, на котором считаем маску.
///
/// `sign-sample.png` — 6280x6484, это 40.7 Мпикс; разворот такого в RGBA плюс
/// промежуточный буфер давал пик 684 МБ и секунду с лишним работы, при том
/// что результат всё равно ужимается до 400 px. Вчетверо больше цели хватает
/// с запасом на чистое уменьшение.
const WORK_MAX: u32 = TARGET_WIDTH * 4;

/// Синие чернила оттиска.
///
/// Светлее и прозрачнее, чем кажется правильным на глаз, и это не эстетика:
/// печать садится поверх фамилии врача, и при плотной заливке фамилия
/// перестаёт читаться. На её настоящих выписках оттиск бледный
/// (замер по скану — RGB 189,199,212), и «Примерова П.П.» видна сквозь него.
pub const INK_BLUE: [u8; 3] = [58, 84, 168];
/// Во сколько раз гасим непрозрачность оттиска, чтобы текст читался насквозь.
const STAMP_ALPHA: f32 = 0.62;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tint {
    /// Перекрасить в синий — для чёрного клише печати.
    Blue,
    /// Оставить цвет как есть — подпись уже синяя.
    Keep,
}

pub fn to_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .context("не закодировать PNG")?;
    Ok(out.into_inner())
}

/// Загрузить что угодно: картинку или PDF из сканера айфона.
///
/// Разбор PDF отдан `package`: он достаёт страницы постранично и, главное,
/// умеет цепочки фильтров. Здесь раньше был свой разбор, и он брал последний
/// фильтр из `/Filter [/FlateDecode /DCTDecode]` — то есть скармливал
/// JPEG-декодеру ещё сжатые Flate байты. На «сжатых» PDF (а так лежат файлы
/// другого пациента и тестового пациента) это давало «в PDF не нашлось картинки».
pub fn load(path: &std::path::Path) -> Result<DynamicImage> {
    let bytes = std::fs::read(path).with_context(|| format!("не прочитать {}", path.display()))?;
    if bytes.starts_with(b"%PDF") {
        let source = crate::package::Source {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            bytes,
            kind: crate::package::Kind::Attachment,
        };
        let page = crate::package::pages(&source)?
            .into_iter()
            .next()
            .context("в этом PDF нет ни одной страницы")?;
        Ok(DynamicImage::ImageRgb8(page))
    } else {
        image::load_from_memory(&bytes).context("не похоже на картинку")
    }
}

/// Основной проход: бумага -> прозрачность, чернила -> непрозрачность.
pub fn extract(src: &DynamicImage, threshold: u8, tint: Tint) -> Result<RgbaImage> {
    // Считаем маску на уменьшенной копии: экономит и память, и время,
    // а итог всё равно ужимается до TARGET_WIDTH.
    let src = if src.width().max(src.height()) > WORK_MAX {
        src.resize(WORK_MAX, WORK_MAX, image::imageops::FilterType::Triangle)
    } else {
        src.clone()
    };

    let rgba = src.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        bail!("пустая картинка");
    }

    // 1. Если альфа уже есть, сначала кладём на белое — иначе прозрачные
    //    участки с чёрным RGB станут «чернилами».
    let flat: Vec<[u8; 3]> = rgba
        .pixels()
        .map(|p| {
            let a = p[3] as u32;
            let over = |c: u8| (((c as u32 * a) + 255 * (255 - a)) / 255) as u8;
            [over(p[0]), over(p[1]), over(p[2])]
        })
        .collect();

    // 2. Две опорные точки: бумага и самые плотные чернила.
    //    Растягивать между ними обязательно — шариковая ручка светло-синяя,
    //    и линейное 255-luma дало бы еле видный росчерк, а клише чёрное.
    let mut hist = [0u32; 256];
    for px in &flat {
        hist[luma(px) as usize] += 1;
    }
    let total: u32 = hist.iter().sum();
    let pct = |p: f64| -> u8 {
        let mut acc = 0u32;
        for (v, n) in hist.iter().enumerate() {
            acc += n;
            if acc as f64 >= total as f64 * p {
                return v as u8;
            }
        }
        255
    };
    // Чернил на листе доли процента, поэтому опорная точка берётся глубоко в хвосте.
    // Замер по присланным файлам: клише 0, подпись 78, скан 162 — при бумаге 255.
    // На 2% хвост попадал уже в бумагу, и весь лист становился чернилами.
    let white = pct(0.95).max(1);
    let dark = pct(0.0005);
    if white.saturating_sub(dark) < 30 {
        bail!("на снимке не видно чернил — проверьте, что сфотографирован нужный лист");
    }
    let span = (white - dark) as f32;

    // 3. Яркость -> альфа с растяжкой, порог убирает фактуру.
    let mut out: RgbaImage = ImageBuffer::new(w, h);
    for (i, px) in flat.iter().enumerate() {
        let a = (((white as f32 - luma(px) as f32) / span) * 255.0).clamp(0.0, 255.0) as u8;
        if a < threshold {
            continue; // прозрачно
        }
        let (rgb, a) = match tint {
            Tint::Blue => (INK_BLUE, (a as f32 * STAMP_ALPHA) as u8),
            Tint::Keep => (*px, a),
        };
        if a == 0 {
            continue;
        }
        out.put_pixel(
            i as u32 % w,
            i as u32 / w,
            Rgba([rgb[0], rgb[1], rgb[2], a]),
        );
    }

    trim_page_edge(&mut out);

    // 4. Обрезаем по чернилам и ужимаем до рабочего размера.
    let Some((x0, y0, x1, y1)) = bbox(&out) else {
        bail!("на снимке не нашлось чернил — переснимите лист при дневном свете")
    };
    let cropped = DynamicImage::ImageRgba8(out).crop_imm(x0, y0, x1 - x0 + 1, y1 - y0 + 1);
    let scaled = if cropped.width() > TARGET_WIDTH {
        cropped.resize(
            TARGET_WIDTH,
            u32::MAX,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        cropped
    };
    Ok(scaled.to_rgba8())
}

/// Стереть кромку листа, но не саму печать.
///
/// Сканер айфона захватывает край бумаги, и он попадает в чернила сплошной
/// полосой вдоль всей стороны. Круглая же печать касается рамки лишь в четырёх
/// точках: замер на `stamp-sample.jpg` — 5.7% крайней линии против сплошной кромки
/// у скана. Поэтому режем линию, только если она непрозрачна почти целиком.
///
/// Раньше здесь была безусловная обрезка 3% с каждой стороны, и она срезала
/// внешнее кольцо оттиска плоскими хордами — клише вписано в квадрат без полей.
fn trim_page_edge(img: &mut RgbaImage) {
    // Замер на скане айфона после порога: верх 63%, низ 92%, лево 13%, право 48%,
    // причём слева кромка сдвинута на пиксель внутрь (столбец 1 — 71%).
    // У круглой печати на той же глубине полосы хорда даёт 27% — запас есть.
    const SOLID: f64 = 0.40;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let depth = ((w.min(h) as f64 * 0.02) as u32).max(1);

    let blank = Rgba([0, 0, 0, 0]);
    // Проходим всю приграничную полосу и гасим каждую сплошную линию:
    // останавливаться на первой несплошной нельзя — кромка бывает не вплотную к краю.
    for i in 0..depth.min(h / 2) {
        for y in [i, h - 1 - i] {
            let share = (0..w).filter(|&x| img.get_pixel(x, y)[3] > 0).count() as f64 / w as f64;
            if share >= SOLID {
                (0..w).for_each(|x| img.put_pixel(x, y, blank));
            }
        }
    }
    for i in 0..depth.min(w / 2) {
        for x in [i, w - 1 - i] {
            let share = (0..h).filter(|&y| img.get_pixel(x, y)[3] > 0).count() as f64 / h as f64;
            if share >= SOLID {
                (0..h).for_each(|y| img.put_pixel(x, y, blank));
            }
        }
    }
}

fn luma(p: &[u8; 3]) -> u8 {
    ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8
}

/// Границы по плотности, а не по единственному пикселю.
///
/// На сканах остаются одиночные крапины далеко от росчерка. Если считать bbox
/// по ним, рамка раздувается вчетверо (замер на подписи: строки 437..5491
/// вместо 2556..3750), и подпись на документе встаёт мелкой и не на месте.
fn bbox(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let mut rows = vec![0u32; h as usize];
    let mut cols = vec![0u32; w as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 0 {
            rows[y as usize] += 1;
            cols[x as usize] += 1;
        }
    }
    let span = |v: &[u32]| -> Option<(u32, u32)> {
        let peak = *v.iter().max()?;
        if peak == 0 {
            return None;
        }
        let min = (peak / 20).max(1);
        let a = v.iter().position(|&n| n >= min)? as u32;
        let b = v.iter().rposition(|&n| n >= min)? as u32;
        Some((a, b))
    };
    let (y0, y1) = span(&rows)?;
    let (x0, x1) = span(&cols)?;
    Some((x0, y0, x1, y1))
}

/// Доля холста, которую занимают чернила, — по ней видно, вырезалось ли что-то осмысленное.
pub fn ink_coverage(img: &RgbaImage) -> f64 {
    let n = img.width() as f64 * img.height() as f64;
    if n == 0.0 {
        return 0.0;
    }
    img.pixels().filter(|p| p[3] > 0).count() as f64 / n
}

/// Сколько из четырёх углов прозрачны.
///
/// Сплошной фон даёт ноль — это и ловим. Требовать все четыре нельзя:
/// подпись идёт по диагонали и после плотной обрезки упирается в угол,
/// а круглая печать краями касается рамки. Осмысленный порог — половина.
pub fn transparent_corners(img: &RgbaImage) -> usize {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return 0;
    }
    [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
        .iter()
        .filter(|&&(x, y)| img.get_pixel(x, y)[3] == 0)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn check(name: &str, tint: Tint) -> RgbaImage {
        let src = load(&sample(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ink = extract(&src, THRESHOLD_DEFAULT, tint).unwrap_or_else(|e| panic!("{name}: {e}"));

        let corners = transparent_corners(&ink);
        assert!(
            corners >= 2,
            "{name}: прозрачен лишь {corners} угол из 4 — фон не убрался"
        );
        let cov = ink_coverage(&ink);
        assert!(
            cov > 0.01,
            "{name}: чернил почти не осталось ({:.1}%)",
            cov * 100.0
        );
        assert!(
            cov < 0.40,
            "{name}: под чернила ушло {:.0}% холста — фактура бумаги не убралась",
            cov * 100.0
        );
        assert!(
            ink.width() <= TARGET_WIDTH,
            "{name}: не ужалось до рабочего размера"
        );
        ink
    }

    #[test]
    fn stamp_cliche_without_alpha() {
        check("stamp-sample.jpg", Tint::Blue);
    }

    #[test]
    fn signature_png_with_opaque_background() {
        check("sign-sample.png", Tint::Keep);
    }

    #[test]
    fn synthetic_scan_pdf() {
        check("scan.pdf", Tint::Keep);
    }

    #[test]
    fn stamp_is_tinted_blue() {
        let ink = check("stamp-sample.jpg", Tint::Blue);
        let opaque = ink
            .pixels()
            .find(|p| p[3] > 120)
            .expect("должны быть чернила");
        assert_eq!([opaque[0], opaque[1], opaque[2]], INK_BLUE);
        // Оттиск обязан оставаться просвечивающим: под ним фамилия врача.
        let densest = ink.pixels().map(|p| p[3]).max().unwrap();
        assert!(
            densest < 200,
            "оттиск слишком плотный ({densest}), фамилия не прочтётся"
        );
    }

    /// Клише вписано в квадрат без полей, и круг касается рамки в четырёх
    /// точках. Безусловная обрезка кромки срезала кольцо плоскими хордами —
    /// проверяем, что оттиск остаётся круглым.
    #[test]
    fn round_stamp_is_not_clipped_into_a_square() {
        let ink = check("stamp-sample.jpg", Tint::Blue);
        let (w, h) = ink.dimensions();
        // Середины сторон — точки касания круга. Хотя бы часть их должна дожить.
        let touches = [(w / 2, 0), (w / 2, h - 1), (0, h / 2), (w - 1, h / 2)]
            .iter()
            .filter(|&&(x, y)| {
                // допускаем сдвиг на пару пикселей от точной середины
                (-2i32..=2).any(|d| {
                    let (nx, ny) = ((x as i32 + d).clamp(0, w as i32 - 1) as u32, y);
                    ink.get_pixel(nx, ny)[3] > 0
                        || ink.get_pixel(x, (ny as i32).clamp(0, h as i32 - 1) as u32)[3] > 0
                })
            })
            .count();
        assert!(
            touches >= 3,
            "кольцо оттиска срезано: живых точек касания {touches} из 4"
        );
        // Круг: соотношение сторон близко к единице.
        let ratio = w as f64 / h as f64;
        assert!(
            (0.9..=1.1).contains(&ratio),
            "оттиск перестал быть круглым: {ratio:.2}"
        );
    }

    /// Первая страница синтетического PDF читается как изображение.
    #[test]
    fn synthetic_pdf_scan_is_read() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/six-pages.pdf");
        let img = load(&path).expect("синтетический PDF должен читаться");
        assert!(
            img.width() > 500 && img.height() > 500,
            "страница пустая: {}x{}",
            img.width(),
            img.height()
        );
    }

    #[test]
    fn degenerate_image_does_not_panic() {
        let empty: RgbaImage = ImageBuffer::new(0, 0);
        assert_eq!(transparent_corners(&empty), 0);
        assert_eq!(ink_coverage(&empty), 0.0);
    }
}
