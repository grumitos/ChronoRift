//! Estado de Riot Vanguard, solo lectura. ChronoRift nunca inicia, detiene ni configura servicios.

use serde::Serialize;

/// Controlador de kernel de Vanguard. Mientras está cargado, Windows niega el inicio de
/// `League of Legends.exe` de otra instalación (os error 5), incluso con permisos elevados.
const VANGUARD_DRIVER: &str = "vgk";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    Running,
    Stopped,
    Missing,
    Unknown,
}

pub fn vanguard_state() -> ServiceState {
    service_state(VANGUARD_DRIVER)
}

/// Consulta el administrador de servicios solo con permiso de lectura de estado. Cualquier
/// estado distinto de detenido, incluidos los pendientes, cuenta como en ejecución.
#[cfg(windows)]
fn service_state(name: &str) -> ServiceState {
    use std::{iter, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_SERVICE_DOES_NOT_EXIST},
        System::Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
            SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS, SERVICE_STATUS, SERVICE_STOPPED,
        },
    };

    let name: Vec<u16> = name.encode_utf16().chain(iter::once(0)).collect();
    // SAFETY: `name` termina en NUL y vive durante las llamadas, `status` es un búfer válido y
    // cada identificador abierto se cierra una sola vez.
    unsafe {
        let manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return ServiceState::Unknown;
        }
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        let state = if service.is_null() {
            // Se lee antes de cerrar el administrador, que sobrescribe el último error.
            if GetLastError() == ERROR_SERVICE_DOES_NOT_EXIST {
                ServiceState::Missing
            } else {
                ServiceState::Unknown
            }
        } else {
            let mut status = SERVICE_STATUS::default();
            let state = if QueryServiceStatus(service, &mut status) == 0 {
                ServiceState::Unknown
            } else if status.dwCurrentState == SERVICE_STOPPED {
                ServiceState::Stopped
            } else {
                ServiceState::Running
            };
            CloseServiceHandle(service);
            state
        };
        CloseServiceHandle(manager);
        state
    }
}

#[cfg(not(windows))]
fn service_state(_name: &str) -> ServiceState {
    ServiceState::Unknown
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Lee servicios reales con los mismos permisos que la app, sin elevación: Tcpip es un
    /// controlador de kernel como vgk y siempre está cargado, y un nombre inventado no existe.
    /// El estado de vgk depende del equipo, pero debe poder leerse (o faltar) sin permisos.
    #[test]
    fn reads_real_driver_states_without_touching_them() {
        assert_eq!(service_state("Tcpip"), ServiceState::Running);
        assert_eq!(
            service_state("ChronoRiftNoSuchService"),
            ServiceState::Missing
        );
        let vgk = vanguard_state();
        eprintln!("vgk: {vgk:?}");
        assert_ne!(vgk, ServiceState::Unknown);
    }
}
