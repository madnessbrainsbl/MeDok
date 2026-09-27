//! Точка входа. Двойной клик открывает окно — консоли нет.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use vypiska::note;

fn main() {
    // Журнал ведём с первой строчки: если окно не откроется, это будет
    // единственный след того, что вообще происходило. Врач пришлёт файл —
    // и видно, на каком шаге встало.
    vypiska::note_new_session();
    note(&format!(
        "запуск: МеДок {}, автор {}, система {}",
        env!("CARGO_PKG_VERSION"),
        vypiska::АВТОР,
        std::env::consts::OS
    ));
    note(&format!("данные: {}", vypiska::settings::dir().display()));
    окружение();
    vypiska::app::записать_среду();

    // Консоли у врача нет, а значит некуда писать ни панику, ни отказ eframe.
    // Показываем окном — но окно сообщения на Linux рисует та же обвязка
    // рабочего стола, что и диалоги выбора файла, и без неё не появляется.
    // Поэтому то же самое пишем и в журнал: он дойдёт всегда.
    std::panic::set_hook(Box::new(|info| {
        note(&format!("ПАНИКА: {info}"));
        alert(&format!("Программа остановилась.\n\n{info}"));
    }));

    let icon = load_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([880.0, 500.0])
        .with_min_inner_size([680.0, 420.0])
        .with_title("МеДок");
    if let Some(i) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(i));
    }

    note("открываю окно…");
    let run = eframe::run_native(
        "МеДок",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| {
            note("окно создано, собираю интерфейс");
            let mut app = vypiska::app::App::new(cc);
            // «Открыть с помощью» из проводника: направление приходит аргументом.
            if let Some(p) = std::env::args_os().nth(1) {
                app.open_path(std::path::Path::new(&p));
            }
            note("готов к работе");
            Ok(Box::new(app))
        }),
    );

    match run {
        Ok(()) => note("закрыто обычным порядком"),
        Err(e) => {
            note(&format!("ОКНО НЕ ОТКРЫЛОСЬ: {e}"));
            alert(&format!(
                "Окно не открылось: {e}\n\n\
                 Программа рисует через OpenGL. Чаще всего помогает установка \
                 драйвера видеокарты."
            ));
        }
    }
}

/// Всё, что понадобится, чтобы разобраться в отказе на чужой машине, куда
/// не попасть: какой рабочий стол, какая сессия, есть ли вообще экран.
fn окружение() {
    for k in [
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "DISPLAY",
        "WAYLAND_DISPLAY",
    ] {
        if let Ok(v) = std::env::var(k) {
            note(&format!("  {k}={v}"));
        }
    }
}

/// Способ сообщить, когда окна программы ещё (или уже) нет. На Linux может
/// не сработать — см. `note`, поэтому туда пишем в любом случае.
fn alert(text: &str) {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("МеДок")
        .set_description(text)
        .show();
}

/// Иконка окна и панели задач — та же, что у файла.
fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/vypiska.ico");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}
