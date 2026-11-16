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
    ("Anyone can create an account.", "Cualquiera puede crear una cuenta."),
    ("Appearance", "Apariencia"),
    ("Archived", "Archivado"),
    ("Back to sign in", "Volver a iniciar sesión"),
    ("Cancel", "Cancelar"),
    ("Channel", "Canal"),
    ("Channel name", "Nombre del canal"),
    ("Channels", "Canales"),
    ("Close", "Cerrar"),
    ("Close the thread", "Cerrar el hilo"),
    ("Complete task", "Completar tarea"),
    ("Connected", "Conectado"),
    ("Connecting…", "Conectando…"),
    ("Copied.", "Copiado."),
    ("Copy", "Copiar"),
    ("Copy the code", "Copiar el código"),
    ("Create", "Crear"),
    ("Create a channel", "Crear un canal"),
    ("Create a workspace", "Crear un espacio de trabajo"),
    ("Create account", "Crear cuenta"),
    ("Create an account", "Crear una cuenta"),
    ("Create task", "Crear tarea"),
    ("Created", "Creado"),
    ("Dark", "Oscuro"),
    ("Delete", "Eliminar"),
    ("Delete this message?", "¿Eliminar este mensaje?"),
    ("Details", "Detalles"),
    ("Direct messages", "Mensajes directos"),
    ("Disconnected. Retrying in {} s.", "Sin conexión. Reintentando en {} s."),
    ("Display name", "Nombre visible"),
    ("Done", "Completada"),
    ("Edit", "Editar"),
    ("Edit message", "Editar mensaje"),
    ("Encrypted before this device could read this conversation.", "Cifrado antes de que este dispositivo pudiera leer esta conversación."),
    ("Encrypted end to end", "Cifrado de extremo a extremo"),
    ("End-to-end encryption is not available. Use TLS (https) for any server outside your computer.", "No hay cifrado de extremo a extremo. Usa TLS (https) para cualquier servidor fuera de tu ordenador."),
    ("Enter sends, Shift+Enter adds a line", "Intro envía, Mayús+Intro añade una línea"),
    ("Everything that happens here stays on your server.", "Todo lo que pasa aquí se queda en tu servidor."),
    ("Expires after", "Caduca en"),
    ("Failed to send. Click to retry.", "No se pudo enviar. Pulsa para reintentar."),
    ("High contrast", "Alto contraste"),
    ("I understand who can read this task.", "Entiendo quién puede leer esta tarea."),
    ("In progress", "En curso"),
    ("Invitation code", "Código de invitación"),
    ("Invitation code (if you have one)", "Código de invitación (si tienes uno)"),
    ("Invite people", "Invitar a gente"),
    ("Join", "Unirse"),
    ("Join a workspace", "Unirse a un espacio de trabajo"),
    ("Join channel", "Unirse al canal"),
    ("Keep me signed in on this computer", "Mantener la sesión en este ordenador"),
    ("Keyboard shortcuts", "Atajos de teclado"),
    ("Language", "Idioma"),
    ("Leave channel", "Salir del canal"),
    ("Light", "Claro"),
    ("Load more", "Cargar más"),
    ("Load older messages", "Cargar mensajes anteriores"),
    ("Loading…", "Cargando…"),
    ("Log out", "Cerrar sesión"),
    ("Main menu", "Menú principal"),
    ("Maximize", "Maximizar"),
    ("Members", "Miembros"),
    ("Mentions", "Menciones"),
    ("Message #{}", "Mensaje en #{}"),
    ("Message {}", "Mensaje para {}"),
    ("Minimize", "Minimizar"),
    ("Move between channels", "Moverse entre canales"),
    ("New device in this conversation: {}", "Nuevo dispositivo en esta conversación: {}"),
    ("New direct message", "Nuevo mensaje directo"),
    ("New line", "Nueva línea"),
    ("New messages", "Mensajes nuevos"),
    ("No channel selected", "Ningún canal seleccionado"),
    ("No messages yet. Say hello.", "Aún no hay mensajes. Saluda."),
    ("No replies yet", "Aún no hay respuestas"),
    ("No results.", "Sin resultados."),
    ("No tasks yet.", "Todavía no hay tareas."),
    ("Nobody else is here yet. Invite someone first.", "Aún no hay nadie más. Invita a alguien primero."),
    ("Nodalia controls", "Controles de Nodalia"),
    ("Offline", "Sin conexión"),
    ("Online", "En línea"),
    ("Only people with an invitation can register.", "Solo puede registrarse quien tenga invitación."),
    ("Only these devices can read what is said here. The server cannot. Compare fingerprints in person to be sure.", "Solo estos dispositivos pueden leer lo que se dice aquí; el servidor no. Comparad las huellas en persona para estar seguros."),
    ("Open a thread", "Abrir un hilo"),
    ("Open source, self-hosted team chat.", "Chat de equipo de código abierto y alojado por ti."),
    ("Password", "Contraseña"),
    ("People", "Personas"),
    ("Preferences", "Preferencias"),
    ("Private", "Privado"),
    ("Private channel", "Canal privado"),
    ("Reduce motion", "Reducir el movimiento"),
    ("Refresh", "Actualizar"),
    ("Registration is closed on this server.", "El registro está cerrado en este servidor."),
    ("Reopen task", "Reabrir tarea"),
    ("Reply", "Responder"),
    ("Reply in thread", "Responder en el hilo"),
    ("Reply…", "Responder…"),
    ("Restore", "Restaurar"),
    ("Save", "Guardar"),
    ("Search", "Buscar"),
    ("Search messages", "Buscar mensajes"),
    ("Search results", "Resultados de la búsqueda"),
    ("Send", "Enviar"),
    ("Send a message", "Enviar un mensaje"),
    ("Sending…", "Enviando…"),
    ("Server", "Servidor"),
    ("Show or hide the details panel", "Mostrar u ocultar el panel de detalles"),
    ("Shown once. Share it privately: whoever has it can join.", "Se muestra una vez. Compártelo en privado: quien lo tenga puede entrar."),
    ("Sign in", "Iniciar sesión"),
    ("Sign in to Ágora", "Inicia sesión en Ágora"),
    ("Signed out. Sign in again.", "Sesión cerrada. Vuelve a iniciar sesión."),
    ("Someone", "Alguien"),
    ("Start task", "Iniciar tarea"),
    ("System", "Sistema"),
    ("System controls", "Controles del sistema"),
    ("Task title", "Título de la tarea"),
    ("Tasks", "Tareas"),
    ("Tasks are visible to the server and workspace members; they are not end-to-end encrypted.", "Las tareas son visibles para el servidor y los miembros del espacio; no están cifradas de extremo a extremo."),
    ("The details panel", "El panel de detalles"),
    ("The window buttons on Linux.", "Los botones de la ventana en Linux."),
    ("Theme", "Tema"),
    ("This address uses http: passwords and messages travel unencrypted. Use it only on your own computer or network.", "Esta dirección usa http: las contraseñas y los mensajes viajan sin cifrar. Úsala solo en tu ordenador o tu red."),
    ("This is the start of #{}.", "Este es el principio de #{}."),
    ("This is the start of your conversation with {}.", "Este es el principio de tu conversación con {}."),
    ("Thread", "Hilo"),
    ("To do", "Por hacer"),
    ("Topic", "Tema del canal"),
    ("Topic (optional)", "Tema (opcional)"),
    ("Use Refresh to load tasks.", "Usa Actualizar para cargar las tareas."),
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
