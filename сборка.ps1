<#
Сборка боевого exe под Windows 7 — именно она стоит на рабочем месте врача.

Делает три вещи, и все три обязательны:

1. Собирает под цель x86_64-win7-windows-msvc со своей стандартной библиотекой.
   Обычная цель x86_64-pc-windows-msvc с Rust 1.78 рассчитана на Windows 10 и
   статически тянет GetSystemTimePreciseAsFileTime, WaitOnAddress и ProcessPrng,
   которых в семёрке нет: exe не доходит до своего кода, загрузчик показывает
   «Точка входа в процедуру … не найдена в библиотеке DLL kernel32.dll».
   Цель win7 — tier 3, готовой std под неё не существует, отсюда nightly и
   -Z build-std.

2. Переписывает в таблице импорта combase.dll на ole32.dll.
   combase.dll появился только в Windows 8 (август 2012). Крейт windows-sys,
   который приходит через rfd, объявляет CoTaskMemFree именно оттуда, хотя эта
   функция живёт в ole32.dll со времён Windows 2000. Без правки exe на семёрке
   не стартует вовсе. ole32.dll подходит везде: на Win7 отдаёт функцию сам,
   на Win8 и новее перенаправляет в combase.

3. Проверяет, что в импортах не осталось ничего новее Windows 7, и кладёт
   готовый exe туда, откуда его забирают.

Запуск:  pwsh -File сборка.ps1          собрать и разложить
         pwsh -File сборка.ps1 -Только  только пропатчить и разложить готовый
#>
param([switch]$Только)
$ErrorActionPreference = 'Stop'
$корень = Split-Path -Parent $MyInvocation.MyCommand.Path
$cargo  = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$exe    = Join-Path $корень 'target\x86_64-win7-windows-msvc\release\vypiska.exe'

if (-not $Только) {
    & $cargo +nightly build --release --target x86_64-win7-windows-msvc -Z build-std=std,panic_abort
    if ($LASTEXITCODE -ne 0) { throw 'сборка не прошла' }
}
if (-not (Test-Path $exe)) { throw "нет собранного exe: $exe" }

# --- разбор PE и правка импортов -------------------------------------------
$b = [IO.File]::ReadAllBytes($exe)
function U16($o) { [BitConverter]::ToUInt16($b, $o) }
function U32($o) { [BitConverter]::ToUInt32($b, $o) }
function U64($o) { [BitConverter]::ToUInt64($b, $o) }
function Str($o) { $e = $o; while ($b[$e] -ne 0) { $e++ }; [Text.Encoding]::ASCII.GetString($b, $o, $e - $o) }

$pe   = U32 0x3C
$coff = $pe + 4
$opt  = $coff + 20
$секции = @()
$нач = $opt + (U16 ($coff + 16))
for ($i = 0; $i -lt (U16 ($coff + 2)); $i++) {
    $o = $нач + $i * 40
    $секции += [pscustomobject]@{ VA = U32 ($o + 12); VSize = U32 ($o + 8); Raw = U32 ($o + 20); RawSize = U32 ($o + 16) }
}
function Смещение($rva) {
    foreach ($s in $секции) {
        $len = [Math]::Max($s.VSize, $s.RawSize)
        if ($rva -ge $s.VA -and $rva -lt $s.VA + $len) { return $rva - $s.VA + $s.Raw }
    }
    return -1
}

# Всё, чего на Windows 7 нет. Список пополняется, когда появляется зависимость.
$запрещено = @(
    'GetSystemTimePreciseAsFileTime', 'WaitOnAddress', 'WakeByAddressSingle',
    'WakeByAddressAll', 'ProcessPrng', 'GetTempPath2W', 'CopyFile2',
    'GetCurrentThreadStackLimits', 'SetThreadDescription', 'GetDpiForWindow',
    'SetProcessDpiAwarenessContext', 'GetSystemMetricsForDpi', 'AdjustWindowRectExForDpi'
)
$новыеDll = @('combase.dll', 'api-ms-win-core-winrt-l1-1-0.dll', 'kernelbase.dll')

$правок = 0
$беда   = @()
$p = Смещение (U32 ($opt + 120))
while ($true) {
    $rva = U32 ($p + 12)
    if ($rva -eq 0) { break }
    $смName = Смещение $rva
    $dll = Str $смName
    if ($dll -ieq 'combase.dll') {
        $нов = [Text.Encoding]::ASCII.GetBytes('ole32.dll')
        for ($i = 0; $i -lt $нов.Length; $i++) { $b[$смName + $i] = $нов[$i] }
        for ($i = $нов.Length; $i -lt $dll.Length; $i++) { $b[$смName + $i] = 0 }
        "  combase.dll -> ole32.dll"
        $правок++
        $dll = 'ole32.dll'
    }
    elseif ($новыеDll -contains $dll.ToLower()) { $беда += "библиотека $dll — её нет на Windows 7" }

    $ilt = U32 $p; if ($ilt -eq 0) { $ilt = U32 ($p + 16) }
    $t = Смещение $ilt
    if ($t -ge 0) {
        while ($true) {
            $e = U64 $t; if ($e -eq 0) { break }
            if (($e -band 0x8000000000000000) -eq 0) {
                $имя = Str ((Смещение ([uint32]$e)) + 2)
                if ($запрещено -contains $имя) { $беда += "$dll -> $имя — этого нет на Windows 7" }
            }
            $t += 8
        }
    }
    $p += 20
}

if ($правок -gt 0) { [IO.File]::WriteAllBytes($exe, $b); "  импорты переписаны" }
else { "  править нечего" }

if ($беда.Count -gt 0) {
    $беда | Sort-Object -Unique | ForEach-Object { "  ОСТАНОВКА: $_" }
    throw 'exe на Windows 7 не запустится — смотри список выше'
}

# Rust оставляет в panic-сообщениях абсолютные пути к registry и исходникам.
# В релизном exe не должно быть имени учётной записи сборщика и пути проекта.
# Меняем байты на латинские x той же длины: смещения внутри PE сохраняются.
$latin = [Text.Encoding]::Latin1
$текст = $latin.GetString($b)
$очищено = 0
foreach ($путь in @($env:USERPROFILE, $корень)) {
    if (-not $путь) { continue }
    $поиск = $latin.GetString([Text.Encoding]::UTF8.GetBytes($путь))
    $совпадений = [regex]::Matches($текст, [regex]::Escape($поиск)).Count
    if ($совпадений -gt 0) {
        $текст = $текст.Replace($поиск, ('x' * $поиск.Length))
        $очищено += $совпадений
    }
}
if ($очищено -gt 0) {
    $b = $latin.GetBytes($текст)
    [IO.File]::WriteAllBytes($exe, $b)
    "  скрыто локальных путей: $очищено"
}

$куда = Join-Path $корень 'vypiska.exe'
Copy-Item $exe $куда -Force
"  разложено: $куда"
"  размер: {0:N2} МиБ" -f ((Get-Item $exe).Length / 1MB)
"ГОТОВО"
