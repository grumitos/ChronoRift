; Textos del instalador de ChronoRift en inglés (la contraparte de spanish.nsh).
;
; Tauri usa este archivo en lugar de su English.nsh integrado (bundle.windows.nsis.customLanguageFiles)
; y lo incluye al final de installer.nsi, cuando ya existen ${LANG_ENGLISH} y ${PRODUCTNAME}.
; No lleva BOM a propósito: el bundler agrega el suyo al copiarlo, y un BOM doble rompe el script.
;
; Tiene las mismas dos primeras partes que spanish.nsh y cada clave debe estar en los dos archivos:
; la que falte en un idioma queda vacía en el instalador. No lleva la tercera parte (las cadenas
; integradas con ^): las de fábrica del inglés ya tratan al usuario de "you".

; ---------------------------------------------------------------------------------------------------
; 1. Textos de la plantilla de Tauri
; ---------------------------------------------------------------------------------------------------

LangString addOrReinstall ${LANG_ENGLISH} "Reinstall ${PRODUCTNAME}"
LangString alreadyInstalled ${LANG_ENGLISH} "Already installed"
LangString alreadyInstalledLong ${LANG_ENGLISH} "${PRODUCTNAME} ${VERSION} is already installed. Choose what you want to do and click Next to continue."
LangString appRunning ${LANG_ENGLISH} "${PRODUCTNAME} is running. Close it and try again."
LangString appRunningOkKill ${LANG_ENGLISH} "${PRODUCTNAME} is running.$\nClick OK to close it."
LangString chooseMaintenanceOption ${LANG_ENGLISH} "Choose the maintenance operation you want to perform."
LangString choowHowToInstall ${LANG_ENGLISH} "Choose how you want to install ${PRODUCTNAME}."
LangString createDesktop ${LANG_ENGLISH} "Create a desktop shortcut"
LangString dontUninstall ${LANG_ENGLISH} "Do not uninstall"
LangString dontUninstallDowngrade ${LANG_ENGLISH} "Do not uninstall (this installer cannot install an older version without uninstalling first)"
LangString failedToKillApp ${LANG_ENGLISH} "Could not close ${PRODUCTNAME}. Close it and try again."
LangString installingWebview2 ${LANG_ENGLISH} "Installing WebView2..."
LangString newerVersionInstalled ${LANG_ENGLISH} "A newer version of ${PRODUCTNAME} is already installed. Installing an older version is not recommended; if you want to, uninstall the current version first. Choose what you want to do and click Next to continue."
LangString older ${LANG_ENGLISH} "older"
LangString olderOrUnknownVersionInstalled ${LANG_ENGLISH} "An $R4 version of ${PRODUCTNAME} is installed. We recommend uninstalling it before you continue. Choose what you want to do and click Next to continue."
LangString silentDowngrades ${LANG_ENGLISH} "This installer cannot install an older version. The silent installation cannot continue; use the graphical installer.$\n"
LangString unableToUninstall ${LANG_ENGLISH} "Could not uninstall."
LangString uninstallApp ${LANG_ENGLISH} "Uninstall ${PRODUCTNAME}"
LangString uninstallBeforeInstalling ${LANG_ENGLISH} "Uninstall before installing"
LangString unknown ${LANG_ENGLISH} "unknown"
LangString webview2AbortError ${LANG_ENGLISH} "Could not install WebView2. Run the installer again."
LangString webview2DownloadError ${LANG_ENGLISH} "Error: could not download WebView2 - $0"
LangString webview2DownloadSuccess ${LANG_ENGLISH} "The WebView2 installer was downloaded successfully."
LangString webview2Downloading ${LANG_ENGLISH} "Downloading the WebView2 installer..."
LangString webview2InstallError ${LANG_ENGLISH} "Error: the WebView2 installation failed with exit code $1."
LangString webview2InstallSuccess ${LANG_ENGLISH} "WebView2 was installed successfully."
LangString deleteAppData ${LANG_ENGLISH} "Delete the application data"

; ---------------------------------------------------------------------------------------------------
; 2. Páginas del asistente (las enlaza installer.nsi con MUI_WELCOMEPAGE_*, MUI_PAGE_HEADER_*, etc.)
; ---------------------------------------------------------------------------------------------------

; Cancelar durante la instalación.
LangString chronoAbortWarning ${LANG_ENGLISH} "Are you sure you want to quit the ${PRODUCTNAME} setup?"

; Bienvenida. "For your user only" refleja installMode: currentUser; si cambia, ajusta el texto.
LangString chronoWelcomeTitle ${LANG_ENGLISH} "Welcome to ${PRODUCTNAME}"
LangString chronoWelcomeText ${LANG_ENGLISH} "This wizard will install ${PRODUCTNAME} on your computer, for your user only.$\r$\n$\r$\nClick Next to continue."

; Carpeta de instalación.
LangString chronoDirectoryTitle ${LANG_ENGLISH} "Installation folder"
LangString chronoDirectorySubtitle ${LANG_ENGLISH} "Choose where ${PRODUCTNAME} will be installed."
LangString chronoDirectoryText ${LANG_ENGLISH} "${PRODUCTNAME} will be installed in the following folder. To use a different one, click Browse and select it."
LangString chronoDirectoryDestination ${LANG_ENGLISH} "Destination folder"

; Progreso de la instalación.
LangString chronoInstallingTitle ${LANG_ENGLISH} "Installing"
LangString chronoInstallingSubtitle ${LANG_ENGLISH} "Please wait while ${PRODUCTNAME} is installed."
LangString chronoInstalledTitle ${LANG_ENGLISH} "Installation complete"
LangString chronoInstalledSubtitle ${LANG_ENGLISH} "${PRODUCTNAME} was installed successfully."
LangString chronoInstallAbortedTitle ${LANG_ENGLISH} "Installation aborted"
LangString chronoInstallAbortedSubtitle ${LANG_ENGLISH} "The installation did not complete successfully."

; Carpeta del menú Inicio. La página solo aparece si startMenuFolder tiene valor; hoy no se muestra.
LangString chronoStartMenuTitle ${LANG_ENGLISH} "Start menu folder"
LangString chronoStartMenuSubtitle ${LANG_ENGLISH} "Choose a Start menu folder for the ${PRODUCTNAME} shortcuts."
LangString chronoStartMenuText ${LANG_ENGLISH} "Choose the Start menu folder where you want to create the program shortcuts. You can also type a name to create a new folder."

; Final de la instalación. El aviso de reinicio solo sale si la instalación pide reiniciar, y no lo hace.
LangString chronoFinishTitle ${LANG_ENGLISH} "Installation complete"
LangString chronoFinishText ${LANG_ENGLISH} "${PRODUCTNAME} is now installed on your computer.$\r$\n$\r$\nClick Finish to close this wizard."
LangString chronoFinishReboot ${LANG_ENGLISH} "Your computer must be restarted to complete the installation of ${PRODUCTNAME}. Do you want to restart it now?"

; Desinstalador: confirmación.
LangString chronoUninstallSubtitle ${LANG_ENGLISH} "${PRODUCTNAME} will be removed from your computer."
LangString chronoUninstallText ${LANG_ENGLISH} "${PRODUCTNAME} will be removed from the following folder. Click Uninstall to continue."
LangString chronoUninstallLocation ${LANG_ENGLISH} "Installation folder:"

; Desinstalador: progreso.
LangString chronoUninstallingTitle ${LANG_ENGLISH} "Uninstalling"
LangString chronoUninstallingSubtitle ${LANG_ENGLISH} "Please wait while ${PRODUCTNAME} is uninstalled."
LangString chronoUninstalledTitle ${LANG_ENGLISH} "Uninstallation complete"
LangString chronoUninstalledSubtitle ${LANG_ENGLISH} "${PRODUCTNAME} was uninstalled successfully."
LangString chronoUninstallAbortedTitle ${LANG_ENGLISH} "Uninstallation aborted"
LangString chronoUninstallAbortedSubtitle ${LANG_ENGLISH} "The uninstallation did not complete successfully."
