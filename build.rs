//! Иконка и свойства файла. Без них exe показывается пустым листом
//! и пугает: неподписанный безымянный бинарь выглядит как вирус.
//!
//! Это ресурсы PE — понятия, которого у ELF на Linux попросту нет, поэтому
//! `winresource` там даже не в зависимостях (см. Cargo.toml). Раз крейта
//! нет в дереве, на него нельзя ссылаться и в коде — отсюда отдельная
//! функция под `#[cfg(windows)]`, а не только проверка переменной среды.

/// Автор программы. Продублировано в `src/lib.rs` (`АВТОР`): build.rs
/// выполняется раньше самой библиотеки и её константы не видит. Меняешь
/// здесь — поменяй и там, иначе в свойствах файла и в окне будут разные имена.
#[cfg(windows)]
const AVTOR: &str = "madnessbrains";

fn main() {
    #[cfg(windows)]
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    println!("cargo:rerun-if-changed=assets/vypiska.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/vypiska.ico");
    res.set("ProductName", "МеДок");
    res.set(
        "FileDescription",
        "Направление, выписка и обследования в одном PDF",
    );
    // Авторство: свойства файла — то место, где оно уместно и переживёт
    // пересылку. В самом документе выписки подписи разработчика быть не
    // должно: медицинский документ уходит в другое учреждение за подписью
    // врача, и посторонняя фамилия там вызывает вопросы.
    res.set("CompanyName", AVTOR);
    res.set("LegalCopyright", &format!("© 2026 {AVTOR}"));
    res.set("OriginalFilename", "MeDok.exe");
    if let Err(e) = res.compile() {
        println!("cargo:warning=иконку встроить не вышло: {e}");
    }
}
