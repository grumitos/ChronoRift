//! Idioma de los textos que el backend muestra al usuario: errores y mensajes de los comandos.
//!
//! El idioma es global porque los errores se vuelven texto en el borde de cada comando
//! (`to_string()`), lejos de quien conoce los ajustes. Solo la configuración de Tauri en `lib.rs` y
//! el comando que guarda el idioma llaman a [`set`]: nada que corra en las pruebas lo cambia, así
//! que las pruebas ven siempre el español.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Es,
    En,
}

static CURRENT: AtomicU8 = AtomicU8::new(Language::Es as u8);

#[cfg(test)]
thread_local! {
    static OVERRIDE: std::cell::Cell<Option<Language>> = const { std::cell::Cell::new(None) };
}

pub fn current() -> Language {
    #[cfg(test)]
    if let Some(language) = OVERRIDE.with(std::cell::Cell::get) {
        return language;
    }
    match CURRENT.load(Ordering::Relaxed) {
        value if value == Language::En as u8 => Language::En,
        _ => Language::Es,
    }
}

pub fn set(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// Idioma de la interfaz de Windows: español si el idioma principal es el español, sea cual sea la
/// variante (es-MX, es-ES…), e inglés en cualquier otro caso. Fuera de Windows, español. Es la
/// única fuente de este dato: la interfaz lo recibe como `systemLanguage`.
pub fn system_language() -> Language {
    #[cfg(windows)]
    {
        // Parte baja de 10 bits del `LANGID`: el idioma sin la variante regional.
        const PRIMARY_LANGUAGE_MASK: u16 = 0x3FF;
        const LANG_SPANISH: u16 = 0x0A;
        // SAFETY: no recibe punteros ni toca memoria del proceso.
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        if id & PRIMARY_LANGUAGE_MASK == LANG_SPANISH {
            Language::Es
        } else {
            Language::En
        }
    }
    #[cfg(not(windows))]
    Language::Es
}

/// Solo para pruebas que formatean un error en el mismo hilo: no llega a tareas ni a
/// `spawn_blocking`.
#[cfg(test)]
pub fn with_language<T>(language: Language, work: impl FnOnce() -> T) -> T {
    let previous = OVERRIDE.with(|cell| cell.replace(Some(language)));
    let result = work();
    OVERRIDE.with(|cell| cell.set(previous));
    result
}

/// Elige la expresión del idioma actual; solo se evalúa esa. Sirve con `format!`, `write!` o
/// literales: `tr!(write!(f, "No se pudo leer {path}"), write!(f, "Could not read {path}"))`.
#[macro_export]
macro_rules! tr {
    ($es:expr, $en:expr $(,)?) => {
        match $crate::i18n::current() {
            $crate::i18n::Language::Es => $es,
            $crate::i18n::Language::En => $en,
        }
    };
}
