//! Общие элементы оформления.
//!
//! Цель одна: поля не должны «плясать». Всё, что идёт парами
//! «подпись — значение», выкладывается сеткой с фиксированной первой колонкой,
//! иначе длина подписи двигает поле, и форма выглядит собранной наспех.

use eframe::egui;

/// Боковые поля окна. Контент не должен упираться в раму.
pub const PAD: i8 = 18;
/// Отступ между смысловыми блоками.
pub const GAP_SECTION: f32 = 16.0;
/// Отступ между строками внутри блока.
pub const GAP_ROW: f32 = 8.0;
/// Ширина колонки подписей — по самой длинной («Организованность»).
pub const LABEL_W: f32 = 168.0;

/// Приглушённый текст.
///
/// Не `.weak()`: стандартный приглушённый даёт на тёмной теме 2.70:1 при
/// норме 4.5:1 — заголовки блоков и все пояснения оказываются на грани
/// видимости. Замер по пикселям: #5F5F5F на #1B1B1B. Здесь 6.2:1.
pub const MUTED: egui::Color32 = egui::Color32::from_gray(154);

/// Цвет предупреждений. Прежний rgb(196,92,60) давал 4.05:1 — тоже мимо нормы.
pub const WARN: egui::Color32 = egui::Color32::from_rgb(224, 120, 90);

/// Цвет «работает, но не до конца»: янтарный, 8.87:1 на нашем фоне.
///
/// Отдельный от `WARN` намеренно. Красным сказано «не вышло», зелёным —
/// «вышло»; между ними есть третий исход, который без своего цвета врёт в обе
/// стороны: приём снимков поднялся и сам себе отвечает, но телефон до него не
/// доходит. Зелёный тут означал бы «всё хорошо» там, где снимков нет, а
/// красный гнал бы перезапускать исправную программу.
pub const ATTENTION: egui::Color32 = egui::Color32::from_rgb(224, 180, 80);

/// Цвет удачного исхода: 5.49:1, проходит.
pub const OK: egui::Color32 = egui::Color32::from_rgb(90, 160, 110);

/// Заголовок смыслового блока: разрядка, приглушённый, с линейкой под ним.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(GAP_SECTION);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .size(11.0)
            .color(MUTED),
    );
    ui.add_space(2.0);
    ui.separator();
    ui.add_space(GAP_ROW);
}

/// Сетка «подпись слева, поле справа» с общей шириной колонки.
pub fn grid<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, GAP_ROW])
        .min_col_width(LABEL_W)
        .show(ui, add)
        .inner
}

/// Подпись в первой колонке сетки.
pub fn label(ui: &mut egui::Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(0.0);
        ui.label(text);
    });
}

/// Однострочное поле на всю оставшуюся ширину.
pub fn edit(ui: &mut egui::Ui, value: &mut String) -> egui::Response {
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY))
}

/// То же, но с образцом формата в пустом поле.
///
/// Образцы вымышленные: настоящие номера пациентов в интерфейсе не показываем.
pub fn edit_hint(ui: &mut egui::Ui, value: &mut String, sample: &str) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(sample)
            .desired_width(f32::INFINITY),
    )
}

/// Решает, должна ли клавиша заполнить пустое поле текстом подсказки.
///
/// Срабатывает только для Стрелки-вправо и Пробела, и только пока поле
/// пусто — как только там есть хоть символ, обе клавиши обязаны вести себя
/// как обычно (допечатать текст, вставить пробел в середину), иначе врач не
/// сможет ни того, ни другого. Вынесено отдельно от egui, чтобы условие
/// проверялось юнит-тестом без поднятия GUI.
fn hint_key_fills(empty: bool, key: egui::Key) -> bool {
    empty && matches!(key, egui::Key::ArrowRight | egui::Key::Space)
}

/// Поле с подсказкой-фразой, которую можно принять одним нажатием: в пустом
/// поле Стрелка-вправо или Пробел вставляют `phrase` целиком и ставят курсор
/// в конец.
///
/// Годится только для полей, где подсказка — это буквальная, часто
/// повторяющаяся фраза (пример: «активно не предъявляет» в жалобах).
/// НЕ годится для подсказок-примеров и подсказок-форматов (диагноз, анамнез
/// заболевания, свидетельство о рождении, организованность) — их нельзя
/// подставлять одним нажатием, это был бы тихий подлог клинических данных.
/// Такие поля остаются на обычном `edit_hint`.
pub fn edit_hint_fill(ui: &mut egui::Ui, value: &mut String, phrase: &str) -> egui::Response {
    let was_empty = value.trim().is_empty();
    let mut out = egui::TextEdit::singleline(value)
        .hint_text(phrase)
        .desired_width(f32::INFINITY)
        .show(ui);

    if out.response.has_focus() {
        let pressed = ui.input(|i| {
            [egui::Key::ArrowRight, egui::Key::Space]
                .into_iter()
                .find(|&k| i.key_pressed(k))
        });
        if pressed.is_some_and(|k| hint_key_fills(was_empty, k)) {
            *value = phrase.to_string();
            // Курсор в конец — иначе следующая буква попадёт в середину фразы.
            let end = egui::text::CCursor::new(value.chars().count());
            out.state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            out.state.store(ui.ctx(), out.response.id);
        }
    }
    out.response
}

/// Поле под номер: принимает только цифры, разделители расставляются сами.
///
/// В пустом поле стоит структура («___-___-___ __»), чтобы было видно,
/// сколько цифр нужно. Буквы и знаки просто не попадают в значение —
/// форматировщик оставляет от ввода одни цифры.
///
/// Курсор после форматирования ставится не по номеру символа, а по номеру
/// цифры слева от него. Строка после форматирования почти всегда другой
/// длины, чем была, — скобки и дефисы то появляются, то исчезают, — и «тот
/// же символьный индекс» уводил курсор не туда: набор «+7999449» на глазах
/// превращался в «+79949449», следующая цифра каждый раз попадала в чужое
/// место. Здесь считается, сколько цифр было ДО курсора, а после
/// форматирования курсор ищет ту же по счёту цифру заново.
pub fn edit_digits(
    ui: &mut egui::Ui,
    value: &mut String,
    template: &str,
    format: impl Fn(&str) -> String,
) -> egui::Response {
    let mut out = egui::TextEdit::singleline(value)
        .hint_text(template)
        .desired_width(f32::INFINITY)
        .show(ui);
    // Буквы поле не принимает — и должно об этом сказать. Молчащее поле
    // читается как сломанное: врач прислала видео, где она держит клавишу, а
    // в ЧДД, ЧСС, СНИЛС и телефоне пусто, и написала «в некоторые строчки
    // нельзя вписывать». Только цифры здесь — её же требование, а вот
    // тишина в ответ на нажатие — недоработка.
    let letter = value
        .chars()
        .any(|c| !c.is_ascii_digit() && !" ()-+_".contains(c));
    let mark = out.response.id.with("не-цифра");
    let now = ui.input(|i| i.time);
    if letter {
        // Подсказку держим пару секунд после последней буквы: мигнувшая на
        // один кадр не читается вовсе.
        ui.ctx().memory_mut(|m| m.data.insert_temp(mark, now + 2.0));
    }
    if let Some(until) = ui.ctx().memory(|m| m.data.get_temp::<f64>(mark)) {
        if now < until {
            out.response
                .clone()
                .show_tooltip_text("Сюда вписываются только цифры");
            ui.ctx().request_repaint();
        }
    }
    if out.response.changed() {
        let cursor = out
            .cursor_range
            .map(|r| r.primary.index)
            .unwrap_or_else(|| value.chars().count());
        let (new_value, pos) = reformat(value, cursor, format);
        *value = new_value;
        out.state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(pos),
            )));
        out.state.store(ui.ctx(), out.response.id);
    }
    out.response
}

/// Код страны, который программа подставляет сама.
const COUNTRY: &str = "+7";

/// Часть строки, по которой вообще имеет смысл считать цифры, и её смещение
/// от начала.
///
/// Код страны не соответствует ни одному нажатию врача: в набранном его нет,
/// а в отформатированном он есть всегда. Считать цифры по всей строке значит
/// сравнивать разные вещи — курсор после первой же цифры вставал не после
/// неё, а внутрь «+7», и вторая цифра уезжала в код страны. Здесь код страны
/// для счёта просто не существует, по обе стороны одинаково.
fn typed_part(s: &str) -> (&str, usize) {
    // Половина кода страны («+») считается им же: пока врач набрал только
    // плюс, курсор обязан стоять за ним, а не перед.
    let n = if s.starts_with(COUNTRY) {
        COUNTRY.len()
    } else {
        usize::from(s.starts_with('+'))
    };
    // Код страны — сплошь ASCII, поэтому байты и символы тут одно и то же.
    (&s[n..], n)
}

/// Пересчёт значения и курсора после одного нажатия: `value` уже изменено
/// полем (буква вставлена, Backspace или Delete что-то убрал), `cursor` —
/// куда поле поставило курсор само.
///
/// Вся суть вынесена сюда из `edit_digits`, чтобы набор, вставку из буфера и
/// правку в середине можно было проверить тестом, не поднимая окна.
fn reformat(value: &str, cursor: usize, format: impl Fn(&str) -> String) -> (String, usize) {
    let (body, off) = typed_part(value);
    // Сколько цифр стоит слева от курсора — это и есть то единственное, что
    // переживает переформатирование: скобки и дефисы то появляются, то
    // исчезают, и прежний символьный индекс после них означает уже не то место.
    let n = body
        .chars()
        .take(cursor.saturating_sub(off))
        .filter(|c| c.is_ascii_digit())
        .count();
    let out = format(value);
    let (out_body, out_off) = typed_part(&out);
    let pos = out_off + digit_cursor_pos(out_body, n);
    (out, pos)
}

/// Символьная позиция сразу после n-й по счёту цифры в строке. Цифр меньше,
/// чем `n`, — курсор в конец: это бывает, когда форматирование само отбросило
/// хвост (например, лишние цифры сверх длины номера).
fn digit_cursor_pos(s: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut seen = 0;
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_digit() {
            seen += 1;
            if seen == n {
                return i + 1;
            }
        }
    }
    s.chars().count()
}

/// Многострочное поле с образцом.
pub fn edit_multi_hint(
    ui: &mut egui::Ui,
    value: &mut String,
    rows: usize,
    sample: &str,
) -> egui::Response {
    ui.add(
        egui::TextEdit::multiline(value)
            .hint_text(sample)
            .desired_rows(rows)
            .desired_width(f32::INFINITY),
    )
}

/// Многострочное поле на всю ширину.
pub fn edit_multi(ui: &mut egui::Ui, value: &mut String, rows: usize) -> egui::Response {
    ui.add(
        egui::TextEdit::multiline(value)
            .desired_rows(rows)
            .desired_width(f32::INFINITY),
    )
}

/// Многострочное поле с просторным межстрочным интервалом — для длинных
/// связных текстов (текст осмотра, анамнез жизни), которые сплошным комом
/// без разрядки читаются тяжело.
///
/// Межстрочный интервал — единственное, что тут можно менять. Сама строка
/// печатается дословно в PDF выписки (Typst вставляет её как есть в параграф
/// документа, см. `vypiska.rs` и `assets/forma27.typ`), поэтому настоящий
/// перенос строки (`\n`) в значении нельзя вставлять ни при вводе, ни тем
/// более программно — это изменило бы напечатанный медицинский документ.
/// Разрядка задаётся только на слое отрисовки через `layouter`; хранимая
/// строка не трогается вообще.
pub fn edit_multi_roomy(ui: &mut egui::Ui, value: &mut String, rows: usize) -> egui::Response {
    let mut font_id = egui::TextStyle::Body.resolve(ui.style());
    font_id.size += 1.0; // чуть крупнее обычного, только для этого поля
    let line_height = ui.fonts(|f| f.row_height(&font_id)) * 1.4;
    let color = ui.visuals().widgets.inactive.text_color();

    let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = egui::text::LayoutJob::simple(
            buf.as_str().to_owned(),
            font_id.clone(),
            color,
            wrap_width,
        );
        for section in &mut job.sections {
            section.format.line_height = Some(line_height);
        }
        ui.fonts(|f| f.layout_job(job))
    };

    ui.add(
        egui::TextEdit::multiline(value)
            .desired_rows(rows)
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter),
    )
}

/// Пояснение под полем: мелко и приглушённо, чтобы не спорило с содержимым.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(MUTED).size(11.5));
}

pub fn warn(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.5).color(WARN));
}

/// Подпись обязательного поля: точка перед названием, а не только красная
/// строка внизу — иначе врач узнаёт об обязательности уже после заполнения.
pub fn label_required(ui: &mut egui::Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
        ui.label(egui::RichText::new("•").color(WARN));
    });
}

/// Крупнее стандартных 18 pt: минимальная цель по доступности — 24.
pub fn apply_style(ctx: &egui::Context) {
    let mut st = (*ctx.style()).clone();
    st.spacing.interact_size.y = 26.0;
    st.spacing.button_padding = egui::vec2(10.0, 6.0);
    st.spacing.item_spacing.y = 6.0;
    ctx.set_style(st);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_field_right_or_space_fills() {
        assert!(hint_key_fills(true, egui::Key::ArrowRight));
        assert!(hint_key_fills(true, egui::Key::Space));
    }

    #[test]
    fn nonempty_field_never_fills() {
        assert!(!hint_key_fills(false, egui::Key::ArrowRight));
        assert!(!hint_key_fills(false, egui::Key::Space));
    }

    #[test]
    fn empty_field_other_keys_never_fill() {
        assert!(!hint_key_fills(true, egui::Key::Enter));
        assert!(!hint_key_fills(true, egui::Key::Tab));
        assert!(!hint_key_fills(true, egui::Key::A));
        assert!(!hint_key_fills(true, egui::Key::ArrowLeft));
    }

    /// Курсор ищет ту же по счёту цифру заново — не тот же символьный индекс,
    /// которого после форматирования может уже не быть на прежнем месте.
    #[test]
    fn digit_cursor_pos_counts_digits_not_characters() {
        assert_eq!(digit_cursor_pos("123-456-789 64", 0), 0);
        // Шестая цифра «7» стоит в тексте после дефиса, седьмым символом.
        assert_eq!(digit_cursor_pos("123-456-789 64", 6), 7);
        // Цифр запрошено больше, чем есть, — курсор уезжает в конец строки.
        assert_eq!(digit_cursor_pos("123-456-789 64", 99), 14);
        assert_eq!(digit_cursor_pos("", 0), 0);
    }

    /// Код страны в счёт цифр не идёт: `reformat` отдаёт сюда уже урезанную
    /// строку, и первая цифра для счёта — первая цифра самого номера.
    #[test]
    fn the_country_code_is_outside_the_digit_count() {
        assert_eq!(typed_part("+7 (900)-000-00-00"), (" (900)-000-00-00", 2));
        assert_eq!(typed_part("+"), ("", 1));
        assert_eq!(typed_part("123-456-789 64"), ("123-456-789 64", 0));
        assert_eq!(typed_part(""), ("", 0));

        let body = " (900)-000-00-00";
        assert_eq!(digit_cursor_pos(body, 1), 3, "первая цифра номера — «9»");
        assert_eq!(digit_cursor_pos(body, 3), 5, "третья — «0», перед скобкой");
        assert_eq!(digit_cursor_pos(body, 99), 16);
    }

    // Дальше — ввод телефона целиком, без окна. Поле сначала правит строку
    // само (egui вставил символ, Backspace или Delete убрал), и только потом
    // зовёт `reformat`; помощники ниже повторяют ровно эту последовательность.

    /// `cursor` — там, где поле оставило курсор ПОСЛЕ своей правки.
    fn edit(value: &str, cursor: usize, f: impl Fn(&mut Vec<char>)) -> (String, usize) {
        let mut s: Vec<char> = value.chars().collect();
        f(&mut s);
        reformat(&s.iter().collect::<String>(), cursor, crate::mask::phone)
    }

    /// Нажата обычная клавиша: символ встаёт под курсор, курсор — за ним.
    fn press(value: &str, cursor: usize, ch: char) -> (String, usize) {
        edit(value, cursor + 1, |s| s.insert(cursor, ch))
    }

    /// Backspace: убирает символ слева, курсор смещается туда же.
    fn backspace(value: &str, cursor: usize) -> (String, usize) {
        edit(value, cursor - 1, |s| {
            s.remove(cursor - 1);
        })
    }

    /// Delete: убирает символ справа, курсор остаётся на месте.
    fn delete(value: &str, cursor: usize) -> (String, usize) {
        edit(value, cursor, |s| {
            s.remove(cursor);
        })
    }

    /// Набор подряд с пустого поля.
    fn type_all(keys: &str) -> (String, usize) {
        let (mut v, mut c) = (String::new(), 0);
        for ch in keys.chars() {
            let (nv, nc) = press(&v, c, ch);
            v = nv;
            c = nc;
        }
        (v, c)
    }

    /// Главный случай: каждая нажатая цифра встаёт следующей по счёту, а не
    /// уезжает в код страны. На прежнем коде курсор после первой же цифры
    /// оставался внутри «+7», и вторая цифра вклинивалась туда же — «777»
    /// показывалось как «+7 (777)-77».
    #[test]
    fn typing_a_phone_from_empty_appends_one_digit_per_key() {
        assert_eq!(type_all("9"), ("+7 (9".into(), 5));
        assert_eq!(type_all("90"), ("+7 (90".into(), 6));
        assert_eq!(type_all("900"), ("+7 (900".into(), 7));
        assert_eq!(type_all("9000"), ("+7 (900)-0".into(), 10));
        assert_eq!(type_all("9000000000"), ("+7 (900)-000-00-00".into(), 18));
        // то, что она набирала на видео: три семёрки — это три семёрки
        assert_eq!(type_all("777"), ("+7 (777".into(), 7));
    }

    /// Дословно то, что она набирает и на чём жалуется: «+7» руками, а следом
    /// номер. Прежде получалось «+79949449».
    #[test]
    fn typing_the_country_code_by_hand_does_not_double_it() {
        assert_eq!(type_all("+7999449").0, "+7 (999)-449");
        assert_eq!(type_all("+79994491234").0, "+7 (999)-449-12-34");
        // и привычный набор с восьмёрки
        assert_eq!(type_all("89000000000").0, "+7 (900)-000-00-00");
    }

    /// Вставка из буфера: номер приходит целиком за одно изменение, курсор —
    /// в конец, откуда его и продолжат править.
    #[test]
    fn pasting_a_whole_phone_normalises_it_at_once() {
        let done = ("+7 (900)-000-00-00".to_string(), 18);
        assert_eq!(reformat("89000000000", 11, crate::mask::phone), done);
        assert_eq!(reformat("+79000000000", 12, crate::mask::phone), done);
        assert_eq!(reformat("8 (900) 000-00-00", 17, crate::mask::phone), done);
    }

    /// Backspace и Delete в середине убирают ровно одну цифру — ту, что
    /// ожидаешь в любом другом поле, — и оставляют курсор на её месте.
    /// Строка после них одна и та же, разница только в курсоре.
    #[test]
    fn backspace_and_delete_take_the_digit_on_their_own_side() {
        let full = "+7 (900)-000-00-00";
        // курсор внутри группы абонентского номера
        assert_eq!(backspace(full, 12), ("+7 (900)-000-00-0".into(), 11));
        assert_eq!(delete(full, 10), ("+7 (900)-000-00-0".into(), 10));
    }

    /// Последняя цифра стёрта — поле пустеет полностью, а не остаётся с
    /// огрызком «+7 (», который потом уедет в документ.
    #[test]
    fn deleting_the_last_digit_empties_the_field() {
        assert_eq!(backspace("+7 (9", 5), (String::new(), 0));
    }

    /// Курсор поставлен мышкой в середину и набрана цифра.
    #[test]
    fn a_digit_typed_in_the_middle_lands_under_the_cursor() {
        // перед «000» в «+7 (900)-000-00-00»
        let (v, c) = press("+7 (900)-000-00-00", 9, '7');
        assert_eq!(v, "+7 (900)-700-00-00");
        assert_eq!(c, 10, "курсор сразу за набранной семёркой");
    }

    /// Выделен кусок и набрана цифра: поле заменяет выделение одним
    /// изменением, остальное обязано уцелеть.
    #[test]
    fn typing_over_a_selection_replaces_only_it() {
        // выделен код «900», сверху набрана «9»
        let (v, c) = edit("+7 (900)-000-00-00", 5, |s| {
            s.splice(4..7, ['9']);
        });
        assert_eq!(v, "+7 (900)-000-00");
        assert_eq!(c, 5);
    }

    /// Буквы и знаки в значение не попадают — даже вперемешку с цифрами,
    /// когда после каждой из них поле переформатируется заново.
    #[test]
    fn letters_and_signs_never_reach_the_value() {
        assert_eq!(type_all("абв"), (String::new(), 0));
        assert_eq!(type_all("9а0б0"), ("+7 (900".into(), 7));
        assert_eq!(type_all("9-0-0-0"), ("+7 (900)-0".into(), 10));
    }

    /// Больше одиннадцати цифр номер не принимает: лишние нажатия ничего не
    /// портят и не сдвигают курсор.
    #[test]
    fn a_phone_stops_at_eleven_digits() {
        assert_eq!(
            type_all("890000000001234"),
            ("+7 (900)-000-00-00".into(), 18)
        );
        assert_eq!(
            press("+7 (900)-000-00-00", 18, '5'),
            ("+7 (900)-000-00-00".into(), 18)
        );
    }
}
