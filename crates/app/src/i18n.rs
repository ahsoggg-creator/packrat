//! Localization table: every string the UI shows lives here, in Russian and English.
//! `{0}`, `{1}` are positional arguments filled by `tf`.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    pub fn from_code(code: &str) -> Lang {
        if code.eq_ignore_ascii_case("en") {
            Lang::En
        } else {
            Lang::Ru
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::Ru
    }
}

macro_rules! strings {
    ($($key:ident => ($ru:expr, $en:expr),)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Key { $($key),* }

        impl Key {
            #[cfg(test)]
            pub const ALL: &'static [Key] = &[$(Key::$key),*];

            pub fn text(self, lang: Lang) -> &'static str {
                match (self, lang) {
                    $((Key::$key, Lang::Ru) => $ru, (Key::$key, Lang::En) => $en,)*
                }
            }
        }
    };
}

strings! {
    AppTitle => ("Packrat", "Packrat"),
    Open => ("Открыть…", "Open…"),
    Extract => ("Распаковать", "Extract"),
    ExtractSmartHere => ("Умно рядом с архивом", "Smart extract here"),
    ExtractTo => ("Распаковать в папку…", "Extract to folder…"),
    CreateArchive => ("Создать архив…", "Create archive…"),
    CloseArchive => ("Закрыть", "Close"),
    Settings => ("Настройки", "Settings"),
    Update => ("Обновление", "Update"),
    Cancel => ("Отмена", "Cancel"),
    Close => ("Закрыть", "Close"),
    Ok => ("ОК", "OK"),
    Browse => ("Обзор…", "Browse…"),
    WelcomeTitle => ("Перетащите архив в окно", "Drop an archive onto the window"),
    WelcomeHint => (
        "Поддерживаются ZIP, 7z, TAR, TAR.GZ/ZST/XZ/LZ4, а также одиночные GZ, BZ2, XZ, ZST, LZ4",
        "ZIP, 7z, TAR, TAR.GZ/ZST/XZ/LZ4 and single GZ, BZ2, XZ, ZST, LZ4 files are supported"
    ),
    Recent => ("Недавние", "Recent"),
    ColName => ("Имя", "Name"),
    ColSize => ("Размер", "Size"),
    ColPacked => ("Сжато", "Packed"),
    ColRatio => ("Степень", "Ratio"),
    ColModified => ("Изменён", "Modified"),
    ColMethod => ("Метод", "Method"),
    StatusArchive => ("{0} · {1} файлов, {2} папок · {3} → {4}", "{0} · {1} files, {2} folders · {3} → {4}"),
    StatusSelected => ("выбрано: {0}", "selected: {0}"),
    ExtractTitle => ("Распаковка", "Extract"),
    ExtractDestination => ("Куда распаковать", "Destination"),
    SmartExtract => ("Умная распаковка (без вложенных «Папка\\Папка»)", "Smart Extract (no Folder\\Folder nesting)"),
    SelectedOnly => ("Только выбранное ({0})", "Selected only ({0})"),
    WholeArchive => ("Весь архив", "Whole archive"),
    ThreadsAuto => ("авто", "auto"),
    StartExtract => ("Распаковать", "Extract"),
    ConflictsTitle => ("Файлы уже существуют", "Files already exist"),
    ConflictsText => (
        "В папке назначения уже есть {0} файл(ов) с такими именами. Что сделать?",
        "{0} file(s) with the same names already exist at the destination. What should happen?"
    ),
    ConflictSkip => ("Пропустить", "Skip"),
    ConflictReplace => ("Заменить", "Replace"),
    ConflictKeepBoth => ("Оставить оба", "Keep both"),
    Working => ("Выполняется…", "Working…"),
    CurrentItem => ("Сейчас: {0}", "Now: {0}"),
    DoneExtract => ("Готово: {0} файлов, {1} за {2} с", "Done: {0} files, {1} in {2} s"),
    DoneCreate => ("Создан {0}: {1} → {2} за {3} с", "Created {0}: {1} → {2} in {3} s"),
    SkippedExisting => ("Пропущено существующих: {0}", "Skipped existing: {0}"),
    KeptBoth => ("Сохранено под новыми именами: {0}", "Saved under new names: {0}"),
    BlockedEntries => ("Заблокировано небезопасных записей: {0}", "Unsafe entries blocked: {0}"),
    FailedEntries => ("Ошибок: {0}", "Errors: {0}"),
    ShowDetails => ("Подробности", "Details"),
    OpenFolder => ("Открыть папку", "Open folder"),
    Cancelled => ("Отменено", "Cancelled"),
    CreateTitle => ("Создать архив", "Create archive"),
    CreateFormat => ("Формат", "Format"),
    CreateLevel => ("Уровень сжатия", "Compression level"),
    CreateName => ("Файл архива", "Archive file"),
    AddFiles => ("Добавить файлы…", "Add files…"),
    AddFolder => ("Добавить папку…", "Add folder…"),
    RemoveItem => ("Убрать", "Remove"),
    NothingAdded => ("Добавьте файлы или папки (можно перетащить в окно)", "Add files or folders (you can drop them onto the window)"),
    StartCreate => ("Создать", "Create"),
    FormatZip => ("ZIP (deflate) — открывается везде", "ZIP (deflate) — opens everywhere"),
    FormatZipZstd => ("ZIP (zstd) — быстрее, нужен современный архиватор", "ZIP (zstd) — faster, needs a modern archiver"),
    Format7z => ("7z (LZMA2) — лучшее сжатие", "7z (LZMA2) — best compression"),
    FormatTarZst => ("TAR.ZST — быстро", "TAR.ZST — fast"),
    FormatTarLz4 => ("TAR.LZ4 — очень быстро", "TAR.LZ4 — very fast"),
    FormatTarGz => ("TAR.GZ", "TAR.GZ"),
    FormatTarXz => ("TAR.XZ", "TAR.XZ"),
    SettingsTitle => ("Настройки", "Settings"),
    SettingLanguage => ("Язык интерфейса", "Interface language"),
    SettingThreads => ("Потоки для сжатия и распаковки", "Threads for compression and extraction"),
    SettingSmartDefault => ("Умная распаковка по умолчанию", "Smart Extract by default"),
    SettingCheckUpdates => ("Проверять обновления раз в сутки (единственный сетевой запрос)", "Check for updates once a day (the only network request)"),
    Diagnostics => ("Диагностика", "Diagnostics"),
    OpenLogsFolder => ("Открыть папку журналов", "Open logs folder"),
    CopyDiagnostics => ("Скопировать диагностику", "Copy diagnostics"),
    Copied => ("Скопировано в буфер обмена", "Copied to clipboard"),
    InstallSection => ("Установка", "Installation"),
    InstallNow => ("Установить для текущего пользователя", "Install for the current user"),
    Installed => ("Установлено: {0}", "Installed: {0}"),
    AboutText => ("Менеджер архивов для Windows. Лицензия MIT или Apache-2.0.", "Archive manager for Windows. MIT or Apache-2.0 license."),
    UpdateAvailable => ("Доступна версия {0}", "Version {0} is available"),
    UpdateUpToDate => ("Установлена последняя версия", "You have the latest version"),
    UpdateCheck => ("Проверить обновления", "Check for updates"),
    UpdateInstall => ("Обновить и перезапустить", "Update and restart"),
    UpdateDownloading => ("Скачивание…", "Downloading…"),
    UpdateFailed => ("Не удалось обновить: {0}", "Update failed: {0}"),
    UpdateChecking => ("Проверка…", "Checking…"),
    DemoBanner => ("ДЕМО: все данные выдуманные, ничего не читается и не пишется", "DEMO: all data is made up, nothing is read or written"),
    DemoNoWrite => ("В демо-режиме ничего не распаковывается", "Nothing is extracted in demo mode"),
    ErrorGeneric => ("Ошибка: {0}", "Error: {0}"),
    DropToAdd => ("Отпустите, чтобы добавить в архив", "Release to add to an archive"),
    CrashReportFound => ("В прошлый раз Packrat завершился сбоем. Отчёт: {0}", "Packrat crashed last time. Report: {0}"),
    ContextMenuHint => ("Правый клик по архиву → Показать дополнительные параметры → Packrat", "Right-click an archive → Show more options → Packrat"),
    Expand => ("Развернуть всё", "Expand all"),
    Collapse => ("Свернуть всё", "Collapse all"),
    DragTooLarge => ("Для перетаскивания слишком много данных (больше {0}): используйте «Распаковать»", "Too much data to drag out (over {0}): use Extract"),
    UninstallConfirm => ("Удалить Packrat из", "Uninstall Packrat from"),
    Uninstalled => ("Packrat удалён.", "Packrat has been uninstalled."),
}

pub fn t(key: Key) -> &'static str {
    key.text(lang())
}

/// Translated text with `{0}`, `{1}`, … replaced by `args`.
pub fn tf(key: Key, args: &[&str]) -> String {
    let mut s = t(key).to_owned();
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .match_indices('{')
            .filter_map(|(i, _)| s[i..].find('}').map(|j| s[i..=i + j].to_owned()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn every_key_has_both_languages_with_matching_placeholders() {
        for k in Key::ALL {
            let (ru, en) = (k.text(Lang::Ru), k.text(Lang::En));
            assert!(!ru.is_empty() && !en.is_empty(), "{k:?} is empty");
            assert_eq!(placeholders(ru), placeholders(en), "{k:?}: placeholders differ");
        }
    }

    #[test]
    fn formatting_fills_positions() {
        set_lang(Lang::En);
        assert_eq!(tf(Key::SkippedExisting, &["5"]), "Skipped existing: 5");
        set_lang(Lang::Ru);
        assert_eq!(tf(Key::SkippedExisting, &["5"]), "Пропущено существующих: 5");
    }
}
