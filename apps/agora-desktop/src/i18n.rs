//! Interface text in Spanish and English.
//!
//! The source is written in English, through [`tr`]; Spanish comes from [`SPANISH`]. A test reads
//! every `tr("…")` in `src/` and fails when one has no Spanish entry, so a new string cannot ship
//! untranslated. Text with a value in it uses `{}` placeholders filled by [`trf`].

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    Es,
    En,
}

impl Lang {
    /// The system language when it is Spanish, English otherwise. macOS and Windows keep the
    /// language in their own settings (an app opened from the Dock has no `LANG`), so ask them;
    /// Linux and the BSDs say it in the environment.
    pub fn system() -> Lang {
        if system_tag().is_some_and(|tag| is_spanish(&tag)) {
            Lang::Es
        } else {
            Lang::En
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Es => "Español",
            Lang::En => "English",
        }
    }
}

fn system_tag() -> Option<String> {
    let platform = || sys_locale::get_locale().filter(|tag| !tag.is_empty() && tag.len() <= 64);
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return platform();
    }
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .or_else(platform)
}

/// Whether a locale tag (`es`, `es-ES`, `es_MX.UTF-8`, `es-419`) names Spanish.
fn is_spanish(tag: &str) -> bool {
    let tag = tag.trim().to_ascii_lowercase();
    tag.strip_prefix("es")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_', '.', '@']))
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::Es => 0,
            Lang::En => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::Es,
        _ => Lang::En,
    }
}

/// The text in the current language.
pub fn tr(english: &'static str) -> &'static str {
    if current() == Lang::En {
        return english;
    }
    SPANISH
        .binary_search_by(|(en, _)| en.cmp(&english))
        .ok()
        .and_then(|i| SPANISH.get(i))
        .map_or(english, |(_, es)| es)
}

/// [`tr`] with each `{}` replaced, in order, by `args`.
pub fn trf(english: &'static str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = tr(english);
    for arg in args {
        match rest.split_once("{}") {
            Some((before, after)) => {
                out.push_str(before);
                out.push_str(arg);
                rest = after;
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// English → Spanish, sorted by the English text (the lookup is a binary search).
const SPANISH: &[(&str, &str)] = &[
    ("(edited)", "(editado)"),
    ("1 member", "1 miembro"),
    ("1 reply", "1 respuesta"),
    ("A new message", "Un mensaje nuevo"),
    ("About Ágora", "Acerca de Ágora"),
    ("Accent, typefaces, icons and window controls come from the Nodalia Design Language.", "Acento, tipografías, iconos y controles de ventana vienen del Nodalia Design Language."),
    ("Account", "Cuenta"),
    ("Add a reaction", "Añadir una reacción"),
    ("Admin", "Administrador"),
    ("Anyone can create an account.", "Cualquiera puede crear una cuenta."),
    ("Appearance", "Apariencia"),
    ("Archived", "Archivado"),
    ("Back", "Atrás"),
    ("Back to sign in", "Volver a iniciar sesión"),
    ("Build or check", "Compilación o comprobación"),
    ("Cancel", "Cancelar"),
    ("Changing an organization role does not change existing workspace access.", "Cambiar el rol de empresa no modifica los accesos existentes a espacios."),
    ("Channel", "Canal"),
    ("Channel name", "Nombre del canal"),
    ("Channels", "Canales"),
    ("Close", "Cerrar"),
    ("Close the thread", "Cerrar el hilo"),
    ("Closed", "Cerrado"),
    ("Code from your authenticator app", "Código de tu app de autenticación"),
    ("Comment", "Comentario"),
    ("Companies contain separate workspaces. Membership alone does not grant access to every workspace.", "Las empresas contienen espacios independientes. Pertenecer a una empresa no da acceso a todos sus espacios."),
    ("Completed", "Completado"),
    ("Connected", "Conectado"),
    ("Connecting…", "Conectando…"),
    ("Connection test", "Prueba de conexión"),
    ("Continue", "Continuar"),
    ("Copied.", "Copiado."),
    ("Copy", "Copiar"),
    ("Copy the code", "Copiar el código"),
    ("Could not load GitHub activity.", "No se pudo cargar la actividad de GitHub."),
    ("Could not load organization details. Select it to retry.", "No se pudieron cargar los detalles. Selecciona la empresa para reintentar."),
    ("Could not load organizations. Close and try again.", "No se pudieron cargar las empresas. Cierra y vuelve a intentarlo."),
    ("Create", "Crear"),
    ("Create a channel", "Crear un canal"),
    ("Create a workspace", "Crear un espacio de trabajo"),
    ("Create account", "Crear cuenta"),
    ("Create an account", "Crear una cuenta"),
    ("Create organization", "Crear empresa"),
    ("Create workspace in organization", "Crear espacio en la empresa"),
    ("Created", "Creado"),
    ("Dark", "Oscuro"),
    ("Delete", "Eliminar"),
    ("Delete this message?", "¿Eliminar este mensaje?"),
    ("Deleted", "Eliminado"),
    ("Delivered · read by {} of {}", "Entregado · leído por {} de {}"),
    ("Details", "Detalles"),
    ("Direct messages", "Mensajes directos"),
    ("Disconnected. Retrying in {} s.", "Sin conexión. Reintentando en {} s."),
    ("Display name", "Nombre visible"),
    ("Done", "Hecho"),
    ("Each computer you sign in on can read your conversations. Logging out keeps a computer's place; remove the ones you no longer use.", "Cada ordenador en el que inicias sesión puede leer tus conversaciones. Cerrar sesión conserva el sitio de ese ordenador; quita los que ya no uses."),
    ("Edit", "Editar"),
    ("Edit message", "Editar mensaje"),
    ("Edited", "Editado"),
    ("Encrypted before this device could read this conversation.", "Cifrado antes de que este dispositivo pudiera leer esta conversación."),
    ("Encrypted end to end", "Cifrado de extremo a extremo"),
    ("Enter a current code. Your old recovery codes stop working.", "Escribe un código actual. Tus códigos de recuperación anteriores dejarán de funcionar."),
    ("Enter sends, Shift+Enter adds a line", "Intro envía, Mayús+Intro añade una línea"),
    ("Enter your password and a current code to turn two-step verification off.", "Escribe tu contraseña y un código actual para desactivar la verificación en dos pasos."),
    ("Enter your password to continue.", "Escribe tu contraseña para continuar."),
    ("Everything that happens here stays on your server.", "Todo lo que pasa aquí se queda en tu servidor."),
    ("Expires after", "Caduca en"),
    ("Failed to send. Click to retry.", "No se pudo enviar. Pulsa para reintentar."),
    ("GitHub activity", "Actividad de GitHub"),
    ("GitHub activity is visible to this workspace and the server. It is not end-to-end encrypted.", "La actividad de GitHub es visible para este espacio y el servidor. No está cifrada de extremo a extremo."),
    ("Guest", "Invitado"),
    ("High contrast", "Alto contraste"),
    ("Invitation code", "Código de invitación"),
    ("Invitation code (if you have one)", "Código de invitación (si tienes uno)"),
    ("Invite people", "Invitar a gente"),
    ("Issue", "Incidencia"),
    ("Join", "Unirse"),
    ("Join a workspace", "Unirse a un espacio de trabajo"),
    ("Join channel", "Unirse al canal"),
    ("Keep me signed in on this computer", "Mantener la sesión en este ordenador"),
    ("Keep these recovery codes somewhere safe. Each one signs you in once if you lose your authenticator app. They are shown only now.", "Guarda estos códigos de recuperación en un lugar seguro. Cada uno permite iniciar sesión una vez si pierdes tu app de autenticación. Solo se muestran ahora."),
    ("Keyboard shortcuts", "Atajos de teclado"),
    ("Language", "Idioma"),
    ("Last seen {}", "Visto por última vez el {}"),
    ("Leave channel", "Salir del canal"),
    ("Light", "Claro"),
    ("Load older messages", "Cargar mensajes anteriores"),
    ("Loading…", "Cargando…"),
    ("Log out", "Cerrar sesión"),
    ("Log out and forget this computer", "Cerrar sesión y olvidar este equipo"),
    ("Main menu", "Menú principal"),
    ("Maximize", "Maximizar"),
    ("Member", "Miembro"),
    ("Members", "Miembros"),
    ("Mentions", "Menciones"),
    ("Message #{}", "Mensaje en #{}"),
    ("Message {}", "Mensaje para {}"),
    ("Messages are end-to-end encrypted with MLS: the server stores only ciphertext. Use TLS (https) for any server outside your computer.", "Los mensajes van cifrados de extremo a extremo con MLS: el servidor solo guarda texto cifrado. Usa TLS (https) para cualquier servidor fuera de tu ordenador."),
    ("Minimize", "Minimizar"),
    ("Move between channels", "Moverse entre canales"),
    ("New device in this conversation: {}", "Nuevo dispositivo en esta conversación: {}"),
    ("New direct message", "Nuevo mensaje directo"),
    ("New line", "Nueva línea"),
    ("New messages", "Mensajes nuevos"),
    ("New recovery codes", "Nuevos códigos de recuperación"),
    ("No GitHub activity yet, or the integration is not configured.", "Todavía no hay actividad de GitHub o la integración no está configurada."),
    ("No channel selected", "Ningún canal seleccionado"),
    ("No messages yet. Say hello.", "Aún no hay mensajes. Saluda."),
    ("No organizations yet.", "Todavía no hay empresas."),
    ("No replies yet", "Aún no hay respuestas"),
    ("No results.", "Sin resultados."),
    ("Nobody else is here yet. Invite someone first.", "Aún no hay nadie más. Invita a alguien primero."),
    ("Nodalia controls", "Controles de Nodalia"),
    ("Notifications", "Notificaciones"),
    ("Notifications never include message text or sender.", "Las notificaciones nunca incluyen el texto ni el remitente."),
    ("Notify me about new messages", "Avisarme de mensajes nuevos"),
    ("Off. Add a code from an authenticator app to your password, so a stolen password is not enough to sign in.", "Desactivada. Añade a tu contraseña un código de una app de autenticación, para que una contraseña robada no baste para entrar."),
    ("Offline", "Sin conexión"),
    ("On. Signing in asks for a code from your authenticator app. Recovery codes left: {}.", "Activada. Al iniciar sesión se pide un código de tu app de autenticación. Códigos de recuperación restantes: {}."),
    ("Online", "En línea"),
    ("Only people with an invitation can register.", "Solo puede registrarse quien tenga invitación."),
    ("Only the owner can manage this organization; other organization roles have the same permissions.", "Solo el propietario puede administrar esta empresa; los demás roles de empresa tienen los mismos permisos."),
    ("Only these devices can read what is said here. The server cannot. Compare fingerprints in person to be sure.", "Solo estos dispositivos pueden leer lo que se dice aquí; el servidor no. Comparad las huellas en persona para estar seguros."),
    ("Open a thread", "Abrir un hilo"),
    ("Open source, self-hosted team chat.", "Chat de equipo de código abierto y alojado por ti."),
    ("Opened", "Abierto"),
    ("Organization members", "Miembros de la empresa"),
    ("Organization name", "Nombre de la empresa"),
    ("Organizations", "Empresas"),
    ("Owner", "Propietario"),
    ("Password", "Contraseña"),
    ("People", "Personas"),
    ("Preferences", "Preferencias"),
    ("Private", "Privado"),
    ("Private channel", "Canal privado"),
    ("Published", "Publicado"),
    ("Pull request", "Solicitud de cambios"),
    ("Read", "Leído"),
    ("Recovery code", "Código de recuperación"),
    ("Reduce motion", "Reducir el movimiento"),
    ("Refresh", "Actualizar"),
    ("Registration is closed on this server.", "El registro está cerrado en este servidor."),
    ("Release", "Versión"),
    ("Remove", "Quitar"),
    ("Reopened", "Reabierto"),
    ("Reply", "Responder"),
    ("Reply in thread", "Responder en el hilo"),
    ("Reply…", "Responder…"),
    ("Repository change", "Cambio en el repositorio"),
    ("Restore", "Restaurar"),
    ("Review", "Revisión"),
    ("Save", "Guardar"),
    ("Scan this code with an authenticator app (any app that supports TOTP), or type the key. Then enter the six digits it shows.", "Escanea este código con una app de autenticación (cualquiera compatible con TOTP) o escribe la clave. Después introduce los seis dígitos que muestre."),
    ("Search", "Buscar"),
    ("Search messages", "Buscar mensajes"),
    ("Search results", "Resultados de la búsqueda"),
    ("Security alert", "Alerta de seguridad"),
    ("Security and devices", "Seguridad y dispositivos"),
    ("Send", "Enviar"),
    ("Send a message", "Enviar un mensaje"),
    ("Sending…", "Enviando…"),
    ("Sent", "Enviado"),
    ("Sent · delivered to {} of {}", "Enviado · entregado a {} de {}"),
    ("Server", "Servidor"),
    ("Show or hide the details panel", "Mostrar u ocultar el panel de detalles"),
    ("Shown once. Share it privately: whoever has it can join.", "Se muestra una vez. Compártelo en privado: quien lo tenga puede entrar."),
    ("Sign in", "Iniciar sesión"),
    ("Sign in to Ágora", "Inicia sesión en Ágora"),
    ("Signed in", "Con sesión iniciada"),
    ("Signed out", "Sin sesión"),
    ("Signed out. Sign in again.", "Sesión cerrada. Vuelve a iniciar sesión."),
    ("Someone", "Alguien"),
    ("System", "Sistema"),
    ("System controls", "Controles del sistema"),
    ("That code is not valid. Check the time on this computer and try again.", "Ese código no es válido. Comprueba la hora de este ordenador y vuelve a intentarlo."),
    ("The details panel", "El panel de detalles"),
    ("The password is not right.", "La contraseña no es correcta."),
    ("The system notification service is unavailable.", "El servicio de notificaciones del sistema no está disponible."),
    ("The window buttons on Linux.", "Los botones de la ventana en Linux."),
    ("Theme", "Tema"),
    ("This account uses two-step verification.", "Esta cuenta usa verificación en dos pasos."),
    ("This address uses http: passwords and messages travel unencrypted. Use it only on your own computer or network.", "Esta dirección usa http: las contraseñas y los mensajes viajan sin cifrar. Úsala solo en tu ordenador o tu red."),
    ("This computer", "Este ordenador"),
    ("This is the start of #{}.", "Este es el principio de #{}."),
    ("This is the start of your conversation with {}.", "Este es el principio de tu conversación con {}."),
    ("Thread", "Hilo"),
    ("Too many attempts. Wait a minute.", "Demasiados intentos. Espera un minuto."),
    ("Topic", "Tema del canal"),
    ("Topic (optional)", "Tema (opcional)"),
    ("Turn off", "Desactivar"),
    ("Turn on", "Activar"),
    ("Two-step verification", "Verificación en dos pasos"),
    ("Use a recovery code", "Usar un código de recuperación"),
    ("Use the authenticator app", "Usar la app de autenticación"),
    ("Username", "Usuario"),
    ("Uses", "Usos"),
    ("Version {}", "Versión {}"),
    ("Who can see it", "Quién puede verlo"),
    ("Window", "Ventana"),
    ("Working…", "Procesando…"),
    ("Workspace name", "Nombre del espacio de trabajo"),
    ("Workspaces", "Espacios de trabajo"),
    ("You are not a member of this channel.", "No eres miembro de este canal."),
    ("You don't belong to any workspace yet.", "Aún no perteneces a ningún espacio de trabajo."),
    ("You have a new message.", "Tienes un mensaje nuevo."),
    ("Your devices", "Tus dispositivos"),
    ("deleted message", "mensaje eliminado"),
    ("everyone in the workspace", "todo el espacio de trabajo"),
    ("invited members only", "solo los miembros invitados"),
    ("{} (this device)", "{} (este dispositivo)"),
    ("{} is typing…", "{} está escribiendo…"),
    ("{} members", "{} miembros"),
    ("{} people are typing…", "{} personas están escribiendo…"),
    ("{} replies", "{} respuestas"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_locale_tags_are_recognised() {
        for tag in ["es", "es-ES", "es_ES.UTF-8", "es-419", "ES_mx", "es@euro"] {
            assert!(is_spanish(tag), "{tag}");
        }
        for tag in ["en-US", "en_GB.UTF-8", "et-EE", "", "C", "POSIX", "est"] {
            assert!(!is_spanish(tag), "{tag}");
        }
    }

    #[test]
    fn the_table_is_sorted_without_duplicates() {
        for pair in SPANISH.windows(2) {
            if let [(a, _), (b, _)] = pair {
                assert!(a < b, "{a:?} must come before {b:?} and appear once");
            }
        }
    }

    #[test]
    fn every_interface_string_has_spanish() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.file_name().is_some_and(|n| n == "i18n.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for marker in ["tr(\"", "trf(\""] {
                    for piece in text.split(marker).skip(1) {
                        let Some(end) = piece.find("\"") else {
                            continue;
                        };
                        let english = &piece[..end];
                        if SPANISH
                            .binary_search_by(|(en, _)| en.cmp(&english))
                            .is_err()
                        {
                            missing.push(format!("{}: {english}", path.display()));
                        }
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "untranslated strings:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn placeholders_fill_in_order() {
        set(Lang::Es);
        assert_eq!(trf("{} replies", &["3"]), "3 respuestas");
        assert_eq!(tr("not in the table"), "not in the table");
        set(Lang::En);
        assert_eq!(trf("Message #{}", &["general"]), "Message #general");
        set(Lang::Es);
    }
}
