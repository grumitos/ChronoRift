; Textos del instalador de ChronoRift en español neutro, tratando al usuario de "tú".
;
; Tauri usa este archivo en lugar de su Spanish.nsh integrado (bundle.windows.nsis.customLanguageFiles)
; y lo incluye al final de installer.nsi, cuando ya existen ${LANG_SPANISH} y ${PRODUCTNAME}.
; No lleva BOM a propósito: el bundler agrega el suyo al copiarlo, y un BOM doble rompe el script.
;
; Tiene tres partes:
;   1. Los textos propios de la plantilla de Tauri. Las claves son las de su English.nsh y deben estar todas.
;   2. Los textos de las páginas del asistente, que installer.nsi enlaza con los defines MUI_*.
;   3. Cadenas integradas de NSIS (las que empiezan con ^) que el idioma de fábrica redacta con "usted".

; ---------------------------------------------------------------------------------------------------
; 1. Textos de la plantilla de Tauri
; ---------------------------------------------------------------------------------------------------

LangString addOrReinstall ${LANG_SPANISH} "Reinstalar ${PRODUCTNAME}"
LangString alreadyInstalled ${LANG_SPANISH} "Ya está instalado"
LangString alreadyInstalledLong ${LANG_SPANISH} "${PRODUCTNAME} ${VERSION} ya está instalado. Elige la operación que quieras realizar y haz clic en Siguiente para continuar."
LangString appRunning ${LANG_SPANISH} "${PRODUCTNAME} está abierto. Ciérralo e inténtalo de nuevo."
LangString appRunningOkKill ${LANG_SPANISH} "${PRODUCTNAME} está abierto.$\nHaz clic en Aceptar para cerrarlo."
LangString chooseMaintenanceOption ${LANG_SPANISH} "Elige la operación de mantenimiento que quieras realizar."
LangString choowHowToInstall ${LANG_SPANISH} "Elige cómo quieres instalar ${PRODUCTNAME}."
LangString createDesktop ${LANG_SPANISH} "Crear acceso directo en el escritorio"
LangString dontUninstall ${LANG_SPANISH} "No desinstalar"
LangString dontUninstallDowngrade ${LANG_SPANISH} "No desinstalar (este instalador no permite instalar una versión anterior sin desinstalar)"
LangString failedToKillApp ${LANG_SPANISH} "No se pudo cerrar ${PRODUCTNAME}. Ciérralo e inténtalo de nuevo."
LangString installingWebview2 ${LANG_SPANISH} "Instalando WebView2..."
LangString newerVersionInstalled ${LANG_SPANISH} "Ya hay una versión más reciente de ${PRODUCTNAME} instalada. No se recomienda instalar una versión anterior; si quieres hacerlo, desinstala antes la versión actual. Elige la operación que quieras realizar y haz clic en Siguiente para continuar."
LangString older ${LANG_SPANISH} "anterior"
LangString olderOrUnknownVersionInstalled ${LANG_SPANISH} "Hay una versión $R4 de ${PRODUCTNAME} instalada. Te recomendamos desinstalarla antes de continuar. Elige la operación que quieras realizar y haz clic en Siguiente para continuar."
LangString silentDowngrades ${LANG_SPANISH} "Este instalador no permite instalar una versión anterior. No se puede continuar con la instalación silenciosa; usa el instalador con interfaz gráfica.$\n"
LangString unableToUninstall ${LANG_SPANISH} "No se pudo desinstalar."
LangString uninstallApp ${LANG_SPANISH} "Desinstalar ${PRODUCTNAME}"
LangString uninstallBeforeInstalling ${LANG_SPANISH} "Desinstalar antes de instalar"
LangString unknown ${LANG_SPANISH} "desconocida"
LangString webview2AbortError ${LANG_SPANISH} "No se pudo instalar WebView2. Vuelve a ejecutar el instalador."
LangString webview2DownloadError ${LANG_SPANISH} "Error: no se pudo descargar WebView2 - $0"
LangString webview2DownloadSuccess ${LANG_SPANISH} "El instalador de WebView2 se descargó correctamente."
LangString webview2Downloading ${LANG_SPANISH} "Descargando el instalador de WebView2..."
LangString webview2InstallError ${LANG_SPANISH} "Error: la instalación de WebView2 falló con el código $1."
LangString webview2InstallSuccess ${LANG_SPANISH} "WebView2 se instaló correctamente."
LangString deleteAppData ${LANG_SPANISH} "Eliminar los datos de la aplicación"

; ---------------------------------------------------------------------------------------------------
; 2. Páginas del asistente (las enlaza installer.nsi con MUI_WELCOMEPAGE_*, MUI_PAGE_HEADER_*, etc.)
; ---------------------------------------------------------------------------------------------------

; Cancelar durante la instalación.
LangString chronoAbortWarning ${LANG_SPANISH} "¿Seguro que quieres salir de la instalación de ${PRODUCTNAME}?"

; Bienvenida. "Solo para tu usuario" refleja installMode: currentUser; si cambia, ajusta el texto.
LangString chronoWelcomeTitle ${LANG_SPANISH} "Te damos la bienvenida a ${PRODUCTNAME}"
LangString chronoWelcomeText ${LANG_SPANISH} "Este asistente instalará ${PRODUCTNAME} en tu computadora, solo para tu usuario.$\r$\n$\r$\nHaz clic en Siguiente para continuar."

; Carpeta de instalación.
LangString chronoDirectoryTitle ${LANG_SPANISH} "Carpeta de instalación"
LangString chronoDirectorySubtitle ${LANG_SPANISH} "Elige dónde se instalará ${PRODUCTNAME}."
LangString chronoDirectoryText ${LANG_SPANISH} "${PRODUCTNAME} se instalará en la carpeta siguiente. Para usar otra, haz clic en Examinar y selecciónala."
LangString chronoDirectoryDestination ${LANG_SPANISH} "Carpeta de destino"

; Progreso de la instalación.
LangString chronoInstallingTitle ${LANG_SPANISH} "Instalando"
LangString chronoInstallingSubtitle ${LANG_SPANISH} "Espera mientras se instala ${PRODUCTNAME}."
LangString chronoInstalledTitle ${LANG_SPANISH} "Instalación completada"
LangString chronoInstalledSubtitle ${LANG_SPANISH} "${PRODUCTNAME} se instaló correctamente."
LangString chronoInstallAbortedTitle ${LANG_SPANISH} "Instalación interrumpida"
LangString chronoInstallAbortedSubtitle ${LANG_SPANISH} "La instalación no se completó correctamente."

; Carpeta del menú Inicio. La página solo aparece si startMenuFolder tiene valor; hoy no se muestra.
LangString chronoStartMenuTitle ${LANG_SPANISH} "Carpeta del menú Inicio"
LangString chronoStartMenuSubtitle ${LANG_SPANISH} "Elige la carpeta del menú Inicio para los accesos directos de ${PRODUCTNAME}."
LangString chronoStartMenuText ${LANG_SPANISH} "Elige la carpeta del menú Inicio donde quieras crear los accesos directos del programa. También puedes escribir un nombre para crear una carpeta nueva."

; Final de la instalación. El aviso de reinicio solo sale si la instalación pide reiniciar, y no lo hace.
LangString chronoFinishTitle ${LANG_SPANISH} "Instalación completada"
LangString chronoFinishText ${LANG_SPANISH} "${PRODUCTNAME} ya está instalado en tu computadora.$\r$\n$\r$\nHaz clic en Terminar para cerrar este asistente."
LangString chronoFinishReboot ${LANG_SPANISH} "Hay que reiniciar la computadora para completar la instalación de ${PRODUCTNAME}. ¿Quieres reiniciarla ahora?"

; Desinstalador: confirmación.
LangString chronoUninstallSubtitle ${LANG_SPANISH} "Se quitará ${PRODUCTNAME} de tu computadora."
LangString chronoUninstallText ${LANG_SPANISH} "${PRODUCTNAME} se quitará de la carpeta siguiente. Haz clic en Desinstalar para continuar."
LangString chronoUninstallLocation ${LANG_SPANISH} "Carpeta de instalación:"

; Desinstalador: progreso.
LangString chronoUninstallingTitle ${LANG_SPANISH} "Desinstalando"
LangString chronoUninstallingSubtitle ${LANG_SPANISH} "Espera mientras se desinstala ${PRODUCTNAME}."
LangString chronoUninstalledTitle ${LANG_SPANISH} "Desinstalación completada"
LangString chronoUninstalledSubtitle ${LANG_SPANISH} "${PRODUCTNAME} se desinstaló correctamente."
LangString chronoUninstallAbortedTitle ${LANG_SPANISH} "Desinstalación interrumpida"
LangString chronoUninstallAbortedSubtitle ${LANG_SPANISH} "La desinstalación no se completó correctamente."

; ---------------------------------------------------------------------------------------------------
; 3. Cadenas integradas de NSIS con "usted" ("Presione", "Seleccione")
; ---------------------------------------------------------------------------------------------------

LangString ^ClickNext ${LANG_SPANISH} "Haz clic en Siguiente para continuar."
LangString ^ClickInstall ${LANG_SPANISH} "Haz clic en Instalar para comenzar la instalación."
LangString ^ClickUninstall ${LANG_SPANISH} "Haz clic en Desinstalar para comenzar la desinstalación."

; Cuadro de Examinar.
LangString ^DirBrowseText ${LANG_SPANISH} "Selecciona la carpeta donde se instalará ${PRODUCTNAME}:"

; No se pudo escribir un archivo: los botones son los de Windows (Anular, Reintentar, Omitir).
LangString ^FileError ${LANG_SPANISH} "No se pudo escribir el archivo:$\r$\n$\t$\"$0$\"$\r$\nHaz clic en Anular para cancelar la instalación,$\r$\nen Reintentar para volver a intentarlo$\r$\no en Omitir para ignorar este archivo."
LangString ^FileError_NoIgnore ${LANG_SPANISH} "No se pudo escribir el archivo:$\r$\n$\t$\"$0$\"$\r$\nHaz clic en Reintentar para volver a intentarlo$\r$\no en Cancelar para cancelar la instalación."
