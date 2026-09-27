// Выписка из медицинской карты амбулаторного больного (форма 27).
// Вёрстка медицинской формы 27.
// Значения приходят через sys.inputs, переносы и разбивка на страницы — за Typst.
//
// Начертания элементов формы:
//   * подчёркнуты только ФИО, дата рождения, домашний адрес и объективный осмотр;
//   * полужирны значения ФИО, даты рождения, полиса и диагноза;
//   * телефон, СНИЛС, свидетельство и организованность — обычным;
//   * заголовок формы — прописными обычного начертания, а не полужирным.

#let d = sys.inputs

#set page(
  paper: "a4",
  margin: (left: 2cm, right: 1.5cm, top: 1.5cm, bottom: 1.5cm),
)
// Первый доступный из списка. Имена — это имена СЕМЕЙСТВ, а не файлов, и
// каждое должно попасть сюда, иначе Typst до шрифта не дойдёт: Liberation
// Serif в движок отдавали, в списке его не было, и на Linux бланк набирался
// вшитым Libertinus — другой рисунок, другие метрики, видно на бумаге.
// Порядок: настоящий Times, его метрический близнец, аварийный вшитый.
#set text(font: ("Times New Roman", "Liberation Serif", "Libertinus Serif"), size: 12pt, lang: "ru", hyphenate: false)
#set par(justify: true, leading: 0.5em, spacing: 0.65em)

// Строка «подпись: значение». Пустое значение не печатаем вовсе —
// пустая графа в медицинском документе хуже отсутствующей.
#let row(label, value, bold: false, line: false) = {
  if value != none and value != "" {
    let head = if line { underline[*#label*] } else { [*#label*] }
    block(spacing: 0.3em)[
      #head #if bold [*#value*] else [#value]
    ]
  }
}

// Раздел с заголовком в подбор: «Анамнез заболевания: текст…»
#let section(label, body, bold: false, line: false) = {
  if body != none and body != "" {
    let head = if line { underline[*#label*] } else { [*#label*] }
    block(spacing: 0.9em, above: 1.0em)[
      #head: #if bold [*#body*] else [#body]
    ]
  }
}

#align(center)[*#d.clinic*]
#line(length: 100%, stroke: 0.6pt)
#v(0.45em)
#align(center)[ВЫПИСКА ИЗ МЕДИЦИНСКОЙ КАРТЫ АМБУЛАТОРНОГО БОЛЬНОГО (ФОРМА 27)]
#line(length: 100%, stroke: 0.6pt)
#v(0.6em)

#row("Фамилия, имя, отчество пациента", d.fio, bold: true, line: true)
#row("Дата рождения", d.dob, bold: true, line: true)
#row("Домашний адрес", d.address, line: true)
#row("Контактный телефон:", d.phone)
#row("СНИЛС", d.snils)
#row("Свидетельство о рождении:", d.birth_cert)
#row("Страховой полис: ОМС: серия №", d.policy, bold: true)
#row("Организованность:", d.organized)

#section("Диагноз основной", d.diagnosis, bold: true)
#section("Анамнез жизни", d.anamnesis_life)
#section("Анамнез заболевания", d.anamnesis_disease)
#section("Объективный осмотр", d.exam, line: true)

#if d.referral_note != "" [
  #block(above: 1.0em)[#d.referral_note]
]
#if d.attachments != "" [
  #block(above: 0.8em)[#d.attachments]
]

// Подписной блок держим вместе, чтобы подпись не разорвало между страницами.
// Отбивку сверху держим небольшой: при длинном анамнезе блок целиком уезжал
// на вторую страницу, и она выходила почти пустой.
// Печать садится ПОВЕРХ фамилии, как оттиск на бумаге: в её выписках
// «Примерова П.П.» стоит внутри круга, а не под ним.
#block(breakable: false, above: 1.1em)[
  #grid(
    columns: (1fr, auto),
    align: (left + top, right + top),
    [
      Лечащий врач: \
      #d.date
    ],
    box[
      #d.doctor
      #if d.stamp != none {
        // Размер задаёт врач в настройках: 2.9 см — это замер её оттиска по
        // скану, но клише у каждого своё, и вживую он встал мелковато.
        // Сдвинут вверх и влево: фамилия должна попадать на нижний край круга,
        // а не под середину, иначе её не прочесть даже сквозь бледный оттиск.
        place(center + horizon, dx: -0.7cm, dy: -0.75cm,
              image(d.stamp, width: float(d.stamp_cm) * 1cm))
      }
      #if d.sign != none {
        place(center + horizon, dx: 1.0cm, dy: -0.25cm, image(d.sign, width: 2.6cm))
      }
    ],
  )
  // Заведующей в выписке нет — убрана по просьбе врача: документ уходит за
  // её собственной подписью, вторая подпись в нём лишняя. Значение из
  // направления по-прежнему разбирается и доезжает сюда в d.head_of_dept,
  // так что вернуть строку — это снова три строки здесь, без правок в Rust.
]
