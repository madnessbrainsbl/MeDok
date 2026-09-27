//! Настройки: то, что не меняется от пациента к пациенту.
//!
//! Лежат рядом с exe (программа переносимая), запасной путь — %APPDATA%.
//! Персданные пациентов сюда не попадают.
//!
//! Врачей здесь несколько: программу ставят не только автору, и за одним
//! компьютером работает смена. Общее у всех — учреждение и формулировки
//! шаблонов (в поликлинике они одни на всех). Своё у каждого — фамилия,
//! должность, заведующий отделением, печать и подпись, то есть ровно то,
//! чем документ заверяется.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Один врач — всё, чем подписывается документ.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Doctor {
    /// Фамилия и инициалы — так они встают под подписью в выписке.
    pub name: String,
    pub post: String,
    /// Заведующий отделением: вторая подпись в направлении из поликлиники.
    /// Он разный, и хранить его было негде — теперь лежит у своего врача.
    pub head_of_dept: String,
    /// Клише печати и росчерк — любой файл: фото, скан, PDF из сканера айфона.
    pub stamp_path: String,
    pub sign_path: String,
}

impl Doctor {
    /// Как показывать врача в списке.
    ///
    /// Пустая фамилия — не пустая строка, а прямая просьба заполнить:
    /// безымянный пункт в выпадающем списке выглядит как сбой программы,
    /// а на деле означает, что подписывать документ нечем.
    pub fn title(&self) -> &str {
        let n = self.name.trim();
        if n.is_empty() {
            "врач не указан"
        } else {
            n
        }
    }

    /// Профиль, в котором ничего нет: ни фамилии, ни должности, ни чернил.
    /// Таким он бывает ровно дважды — на свежей установке и сразу после
    /// «Добавить врача».
    pub fn is_blank(&self) -> bool {
        [
            &self.name,
            &self.post,
            &self.head_of_dept,
            &self.stamp_path,
            &self.sign_path,
        ]
        .iter()
        .all(|s| s.trim().is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Читаем через `Stored`: там же лежит и старая плоская форма файла.
#[serde(from = "Stored")]
pub struct Settings {
    pub clinic: String,
    /// Список никогда не пуст: чтение файла всегда оставляет хотя бы один
    /// профиль, а удаление последнего запрещено.
    pub doctors: Vec<Doctor>,
    /// Кем подписывают прямо сейчас. Индекс в `doctors`.
    pub active: usize,
    /// Накладывать печать и подпись прямо в PDF.
    pub overlay: bool,
    /// Постоянный текст осмотра с местами под жалобы, ЧДД и ЧСС.
    pub exam_template: String,
    /// Постоянная заготовка анамнеза жизни — подставляется каждому новому
    /// пациенту, чтобы врач правил прочерки, а не набирал текст заново.
    pub life_template: String,
    /// Ширина оттиска печати в документе, в сантиметрах.
    pub stamp_cm: f32,
}

/// Как настройки лежат в файле.
///
/// Здесь же остались ключи старой плоской формы, когда врач был один на всю
/// программу (`doctor`, `stamp_path`, `sign_path` на верхнем уровне).
/// Выбросить их нельзя: у врача такой файл уже лежит на диске, и потеря путей
/// к печати означает не «настройки сбросились», а документ без печати —
/// заметить это он может уже после того, как выписка ушла.
///
/// Пишем всегда только новую форму: `Serialize` у `Settings` свой.
#[derive(Deserialize)]
#[serde(default)]
struct Stored {
    clinic: String,
    doctors: Vec<Doctor>,
    active: usize,
    overlay: bool,
    exam_template: String,
    life_template: String,
    stamp_cm: f32,

    // Старая форма, только на чтение.
    doctor: String,
    stamp_path: String,
    sign_path: String,
}

/// Значения по умолчанию заданы здесь и только здесь.
///
/// Раздвоить их с `Settings::default()` нельзя: файл без ключа `overlay`
/// читался бы с одним значением, а свежие настройки заводились бы с другим —
/// печать то ставится, то нет, и причину не видно.
impl Default for Stored {
    fn default() -> Self {
        Self {
            clinic: "Тестовая детская поликлиника".into(),
            doctors: Vec::new(),
            active: 0,
            overlay: true,
            exam_template: crate::vypiska::EXAM_DEFAULT.into(),
            life_template: crate::vypiska::LIFE_DEFAULT.into(),
            stamp_cm: crate::vypiska::STAMP_CM_DEFAULT,
            doctor: String::new(),
            stamp_path: String::new(),
            sign_path: String::new(),
        }
    }
}

impl From<Stored> for Settings {
    fn from(s: Stored) -> Self {
        let mut doctors = s.doctors;
        // Список пуст — значит либо файл старой формы, либо первый запуск.
        // В обоих случаях заводим один профиль: в первом он забирает фамилию
        // и пути к чернилам, во втором остаётся пустым под заполнение.
        if doctors.is_empty() {
            doctors.push(Doctor {
                name: s.doctor,
                stamp_path: s.stamp_path,
                sign_path: s.sign_path,
                ..Default::default()
            });
        }
        // Индекс из файла может указывать в пустоту (профиль удалили руками).
        // Тогда подписывает последний существующий, а не никто.
        let active = s.active.min(doctors.len() - 1);
        Self {
            clinic: s.clinic,
            doctors,
            active,
            overlay: s.overlay,
            exam_template: s.exam_template,
            life_template: s.life_template,
            // Правленный руками файл может принести что угодно, вплоть до
            // нуля: оттиск тогда исчезнет, а врач будет искать причину в
            // картинке. Держим в тех же пределах, что и ползунок.
            stamp_cm: s.stamp_cm.clamp(
                *crate::vypiska::STAMP_CM_RANGE.start(),
                *crate::vypiska::STAMP_CM_RANGE.end(),
            ),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Stored::default().into()
    }
}

/// Где держать настройки.
///
/// Сначала — папка рядом с exe: программа переносимая, её носят на флешке,
/// и настройки должны ехать вместе с ней. Если писать туда нельзя (запустили
/// из Program Files или с диска только на чтение) — уходим в системную папку
/// настроек: `%APPDATA%` на Windows, `$XDG_CONFIG_HOME` (обычно `~/.config`)
/// на Linux. `std::env::temp_dir()` как крайний случай — она хотя бы
/// существует всегда, даже если оба обычных пути недоступны.
///
/// Считается один раз: проба записи внутри, и делать её на каждое обращение
/// незачем, а расхождение между двумя вызовами дало бы запись мимо каталога.
pub fn dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(near) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("vypiska-данные")))
        {
            if std::fs::create_dir_all(&near).is_ok() && writable(&near) {
                return near;
            }
        }
        let fallback = config_home().join("vypiska");
        let _ = std::fs::create_dir_all(&fallback);
        fallback
    })
}

/// Системная папка настроек — там, где эту программу не носят на флешке
/// (установили в `Program Files`/`/usr/bin`, диск только на чтение).
#[cfg(windows)]
fn config_home() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

#[cfg(unix)]
fn config_home() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|_| std::env::temp_dir())
}

fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn file() -> PathBuf {
    dir().join("settings.json")
}

impl Settings {
    /// Активный врач — тот, чьими печатью и подписью заверяется документ.
    pub fn doc(&self) -> &Doctor {
        &self.doctors[self.active.min(self.doctors.len() - 1)]
    }

    pub fn doc_mut(&mut self) -> &mut Doctor {
        let i = self.active.min(self.doctors.len() - 1);
        &mut self.doctors[i]
    }

    /// Новый врач сразу становится активным: его и пришли заводить.
    ///
    /// Пустой профиль не размножаем. На свежей установке в списке уже лежит
    /// один пустой, и второе нажатие подряд заводило второй такой же: в
    /// выпадающем списке появлялись два пункта «врач не указан», подписывал
    /// документ тот, что моложе, а удалить лишний врач не догадывалась.
    /// Ровно это и лежит в настройках на рабочей машине: `doctors[0]` пустой,
    /// `active` указывает на второй.
    pub fn add_doctor(&mut self) {
        if self.doc().is_blank() {
            return;
        }
        self.doctors.push(Doctor::default());
        self.active = self.doctors.len() - 1;
    }

    /// Удалить активного врача. Последнего удалить нельзя: без профиля
    /// подписывать нечем, а пустой список сломал бы и шапку окна.
    /// Возвращает, случилось ли удаление.
    pub fn remove_active(&mut self) -> bool {
        if self.doctors.len() < 2 {
            return false;
        }
        let i = self.active.min(self.doctors.len() - 1);
        self.doctors.remove(i);
        self.active = i.min(self.doctors.len() - 1);
        true
    }

    /// Битые или отсутствующие настройки — не повод падать при старте.
    /// Но и затирать нечитаемый файл молча нельзя: рядом кладётся `.bad`,
    /// иначе врач теряет свой текст осмотра без следа.
    pub fn load() -> Self {
        let path = file();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(_) => {
                let _ = std::fs::write(path.with_extension("json.bad"), &raw);
                Self::default()
            }
        }
    }

    /// Пишем через временный файл: обрыв на записи (вынули флешку) не должен
    /// оставлять обрезанный JSON, который при следующем запуске молча
    /// превратится в настройки по умолчанию.
    pub fn save(&self) -> Result<()> {
        let path = file();
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir_all(dir()).context("не создать папку настроек")?;
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .context("не записать настройки")?;
        std::fs::rename(&tmp, &path).context("не заменить файл настроек")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_carry_a_usable_exam_template() {
        let s = Settings::default();
        for ph in ["{жалобы}", "{чдд}", "{чсс}"] {
            assert!(s.exam_template.contains(ph), "в шаблоне нет {ph}");
        }
        assert!(s.overlay, "наложение печати включено по умолчанию");
        assert_eq!(s.doctors.len(), 1, "без профиля подписывать нечем");
        assert!(
            s.doc().name.is_empty(),
            "фамилия врача не придумывается за него"
        );
    }

    #[test]
    fn round_trip_keeps_paths_and_template() {
        let mut s = Settings {
            exam_template: "Жалобы {жалобы}, ЧДД {чдд}, ЧСС {чсс}.".into(),
            ..Default::default()
        };
        *s.doc_mut() = Doctor {
            name: "Примерова П. П.".into(),
            post: "врач-педиатр участковый".into(),
            head_of_dept: "Старшая С. С.".into(),
            stamp_path: "D:/печать.jpg".into(),
            sign_path: "D:/sign-sample.png".into(),
        };
        s.add_doctor();
        s.doc_mut().name = "Руководитель Р. Р.".into();

        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.doctors.len(), 2);
        assert_eq!(back.active, 1, "кем подписывали, тем и продолжаем");
        assert_eq!(back.doc().name, "Руководитель Р. Р.");
        assert_eq!(back.doctors[0].stamp_path, "D:/печать.jpg");
        assert_eq!(back.doctors[0].sign_path, "D:/sign-sample.png");
        assert_eq!(back.doctors[0].post, "врач-педиатр участковый");
        assert_eq!(back.doctors[0].head_of_dept, "Старшая С. С.");
        assert_eq!(back.exam_template, s.exam_template);
        assert!(
            back.clinic.contains("поликлиника"),
            "учреждение общее для всех"
        );
    }

    /// У врача на диске уже лежит файл старой формы. Обновление программы не
    /// имеет права его обнулить: печать и подпись он заново не переложит —
    /// просто выпустит документ без них и не заметит.
    #[test]
    fn old_flat_settings_become_the_first_profile() {
        let raw = r#"{
  "clinic": "Тестовая детская поликлиника",
  "doctor": "Примерова П. П.",
  "stamp_path": "D:\\печати\\печать.jpg",
  "sign_path": "D:\\печати\\sign-sample.png",
  "overlay": true,
  "exam_template": "Жалобы {жалобы}. ЧДД {чдд}, ЧСС {чсс}.",
  "life_template": "Родился доношенным."
}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!(s.doctors.len(), 1, "старый врач стал первым профилем");
        assert_eq!(s.active, 0);
        assert_eq!(s.doc().name, "Примерова П. П.");
        assert_eq!(s.doc().stamp_path, "D:\\печати\\печать.jpg");
        assert_eq!(s.doc().sign_path, "D:\\печати\\sign-sample.png");
        assert_eq!(s.exam_template, "Жалобы {жалобы}. ЧДД {чдд}, ЧСС {чсс}.");
        assert_eq!(s.life_template, "Родился доношенным.");
        assert!(s.clinic.contains("поликлиника"));

        // Обратно на диск уходит уже новая форма: старые ключи не плодим,
        // иначе через версию будет непонятно, какой из двух главнее.
        let again: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(again.get("doctors").is_some(), "врачи пишутся списком");
        assert!(
            again.get("doctor").is_none(),
            "плоский ключ больше не пишется"
        );
        assert!(
            again.get("stamp_path").is_none(),
            "путь к печати переехал к врачу"
        );
    }

    /// Файл настроек правится руками, и лишний/пропущенный ключ не должен
    /// ронять программу на старте.
    #[test]
    fn partial_json_falls_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"doctor":"Иванов И.И.","лишний":1}"#).unwrap();
        assert_eq!(s.doc().name, "Иванов И.И.");
        assert!(s.exam_template.contains("{чдд}"));
        assert!(s.clinic.contains("поликлиника"));
        assert!(s.overlay, "пропущенный ключ не снимает печать с документа");
    }

    /// Индекс активного врача может указывать в пустоту — профиль удалили,
    /// пока файл лежал открытым в блокноте.
    #[test]
    fn active_index_out_of_range_is_clamped() {
        let s: Settings =
            serde_json::from_str(r#"{"doctors":[{"name":"А"},{"name":"Б"}],"active":7}"#).unwrap();
        assert_eq!(s.active, 1);
        assert_eq!(s.doc().name, "Б");
    }

    /// «Добавить врача» на пустом профиле не заводит второй пустой.
    /// Иначе в списке два безымянных пункта, подписывает документ второй —
    /// то есть никто, — и лишний оттуда уже не убрать.
    #[test]
    fn an_empty_profile_is_not_duplicated() {
        let mut s = Settings::default();
        s.add_doctor();
        s.add_doctor();
        assert_eq!(s.doctors.len(), 1, "пустой профиль размножился");
        assert_eq!(s.active, 0);

        // Как только профиль заполнен, кнопка работает как раньше.
        s.doc_mut().name = "Примерова П. П.".into();
        s.add_doctor();
        assert_eq!(s.doctors.len(), 2);
        assert_eq!(s.active, 1);
        assert_eq!(
            s.doctors[0].name, "Примерова П. П.",
            "прежний врач на месте"
        );
    }

    #[test]
    fn the_last_doctor_cannot_be_removed() {
        let mut s = Settings::default();
        s.doc_mut().name = "Примерова П. П.".into();
        assert!(!s.remove_active(), "последний профиль удалять нельзя");
        assert_eq!(s.doctors.len(), 1);
        assert_eq!(s.doc().name, "Примерова П. П.");

        s.add_doctor();
        s.doc_mut().name = "Руководитель Р. Р.".into();
        assert_eq!(s.active, 1, "новый врач сразу активен");
        assert!(s.remove_active());
        assert_eq!(s.doctors.len(), 1);
        assert_eq!(s.active, 0, "активным остался существующий профиль");
        assert_eq!(s.doc().name, "Примерова П. П.");
    }
}
