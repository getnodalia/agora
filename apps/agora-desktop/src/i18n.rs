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

/// The Spanish for an English text that is only known at run time, such as a server's message.
fn lookup(english: &str) -> Option<&'static str> {
    SPANISH
        .binary_search_by(|(en, _)| (*en).cmp(english))
        .ok()
        .and_then(|i| SPANISH.get(i))
        .map(|(_, es)| *es)
}

/// What to tell a person about a failed request, in their language and in plain words: what
/// went wrong and, where there is one, what to do. The server's own message is kept when it
/// says something specific and has a translation (or the interface is English).
pub fn explain(error: &agora_client::ClientError) -> String {
    use agora_client::ClientError as E;
    use agora_protocol::ErrorCode as C;
    let english = current() == Lang::En;
    match error {
        E::Api { error, .. } => {
            if !english {
                if let Some(es) = lookup(&error.message) {
                    return es.to_owned();
                }
            }
            let general = match error.code {
                C::Forbidden => tr("You are not allowed to do that."),
                C::NotFound => tr("That no longer exists, or you cannot see it."),
                C::Unauthenticated => tr("Your session ended. Sign in again."),
                C::RateLimited => tr("Too many requests. Wait a moment and try again."),
                C::Conflict => tr("That already exists."),
                C::RegistrationClosed => tr("This server does not accept new accounts."),
                C::Invalid => tr("That is not valid."),
                _ => "",
            };
            match (english, general.is_empty()) {
                (true, _) | (_, true) => error.message.clone(),
                (false, false) if matches!(error.code, C::Invalid | C::Conflict) => {
                    format!("{general} ({})", error.message)
                }
                (false, false) => general.to_owned(),
            }
        }
        E::Network(_) => tr("Cannot reach the server. Check the address and your connection.").to_owned(),
        E::InvalidUrl(_) => tr("The server address is not valid.").to_owned(),
        E::Decode(_) => tr("The server answered something unexpected. Is it running a compatible version of Ágora?").to_owned(),
        E::Encryption(detail) => trf("Encryption failed: {}", &[detail]),
        E::Local(detail) => detail.clone(),
    }
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
    ("1 person in the call", "1 persona en la llamada"),
    ("1 reply", "1 respuesta"),
    ("A new message", "Un mensaje nuevo"),
    ("About Ágora", "Acerca de Ágora"),
    ("Accent, typefaces, icons and window controls come from the Nodalia Design Language.", "Acento, tipografías, iconos y controles de ventana vienen del Nodalia Design Language."),
    ("Account", "Cuenta"),
    ("Add", "Añadir"),
    ("Add a reaction", "Añadir una reacción"),
    ("Add an integration", "Añadir una integración"),
    ("Address", "Dirección"),
    ("Admin", "Administrador"),
    ("Alerts from your servers and services.", "Alertas de tus servidores y servicios."),
    ("Another app", "Otra app"),
    ("Another app or script", "Otra app o script"),
    ("Any app that can send a signed web request.", "Cualquier app que pueda enviar una petición web firmada."),
    ("Anyone can create an account.", "Cualquiera puede crear una cuenta."),
    ("Anything that can send a signed web request: a build server, monitoring, or your own script.", "Cualquier cosa que pueda enviar una petición web firmada: un servidor de compilación, una monitorización o tu propio script."),
    ("Anything you run yourself, from a cron job to a shell.", "Lo que ejecutes tú, desde una tarea programada hasta una terminal."),
    ("Appearance", "Apariencia"),
    ("Archived", "Archivado"),
    ("At least {} characters.", "Al menos {} caracteres."),
    ("Back", "Atrás"),
    ("Back to sign in", "Volver a iniciar sesión"),
    ("Build or check", "Compilación o comprobación"),
    ("Build server", "Servidor de compilación"),
    ("Builds", "Compilaciones"),
    ("Builds and deployments: started, passed, failed.", "Compilaciones y despliegues: iniciados, correctos, fallidos."),
    ("Calls are end-to-end encrypted.", "Las llamadas están cifradas de extremo a extremo."),
    ("Cancel", "Cancelar"),
    ("Cannot reach the server. Check the address and your connection.", "No se puede conectar con el servidor. Revisa la dirección y tu conexión."),
    ("Change password", "Cambiar contraseña"),
    ("Changing an organization role does not change existing workspace access.", "Cambiar el rol de empresa no modifica los accesos existentes a espacios."),
    ("Channel", "Canal"),
    ("Channel name", "Nombre del canal"),
    ("Channels", "Canales"),
    ("Channels keep a conversation about one subject in one place.", "Los canales mantienen en un solo sitio la conversación sobre un tema."),
    ("Check for updates…", "Buscar actualizaciones…"),
    ("Check it works", "Comprueba que funciona"),
    ("Choose new password", "Elegir contraseña nueva"),
    ("Choose Ágora and paste this address:", "Elige Ágora y pega esta dirección:"),
    ("Close", "Cerrar"),
    ("Close the thread", "Cerrar el hilo"),
    ("Closed", "Cerrado"),
    ("Code from your authenticator app", "Código de tu app de autenticación"),
    ("Comment", "Comentario"),
    ("Companies contain separate workspaces. Membership alone does not grant access to every workspace.", "Las empresas contienen espacios independientes. Pertenecer a una empresa no da acceso a todos sus espacios."),
    ("Completed", "Completado"),
    ("Connect Hito", "Conectar Hito"),
    ("Connect an app", "Conectar una app"),
    ("Connect another app to this channel and its notices appear here as they happen: a Hito project, a build server, a script.", "Conecta otra app a este canal y sus avisos aparecerán aquí al instante: un proyecto de Hito, un servidor de compilación, un script."),
    ("Connect {}", "Conectar {}"),
    ("Connected", "Conectado"),
    ("Connected ({})", "Conectadas ({})"),
    ("Connecting…", "Conectando…"),
    ("Connection created: {}", "Conexión creada: {}"),
    ("Connection test", "Prueba de conexión"),
    ("Continue", "Continuar"),
    ("Conversation", "Conversación"),
    ("Copied.", "Copiado."),
    ("Copy", "Copiar"),
    ("Copy example command", "Copiar comando de ejemplo"),
    ("Copy the code", "Copiar el código"),
    ("Could not load GitHub activity.", "No se pudo cargar la actividad de GitHub."),
    ("Could not load organization details. Select it to retry.", "No se pudieron cargar los detalles. Selecciona la empresa para reintentar."),
    ("Could not load organizations. Close and try again.", "No se pudieron cargar las empresas. Cierra y vuelve a intentarlo."),
    ("Create", "Crear"),
    ("Create a channel", "Crear un canal"),
    ("Create a recovery key", "Crear una clave de recuperación"),
    ("Create a workspace", "Crear un espacio de trabajo"),
    ("Create account", "Crear cuenta"),
    ("Create an account", "Crear una cuenta"),
    ("Create connection", "Crear conexión"),
    ("Create organization", "Crear empresa"),
    ("Create workspace in organization", "Crear espacio en la empresa"),
    ("Created", "Creado"),
    ("Current password", "Contraseña actual"),
    ("Dark", "Oscuro"),
    ("Delete", "Eliminar"),
    ("Delete this message?", "¿Eliminar este mensaje?"),
    ("Delete this workspace?", "¿Eliminar este espacio de trabajo?"),
    ("Delete this workspace…", "Eliminar este espacio de trabajo…"),
    ("Delete workspace", "Eliminar espacio de trabajo"),
    ("Deleted", "Eliminado"),
    ("Delivered · read by {} of {}", "Entregado · leído por {} de {}"),
    ("Details", "Detalles"),
    ("Direct messages", "Mensajes directos"),
    ("Disconnected. Retrying in {} s.", "Sin conexión. Reintentando en {} s."),
    ("Display name", "Nombre visible"),
    ("Done", "Hecho"),
    ("Drop the files to send them", "Suelta los archivos para enviarlos"),
    ("Each computer you sign in on can read your conversations. Logging out keeps a computer's place; remove the ones you no longer use.", "Cada ordenador en el que inicias sesión puede leer tus conversaciones. Cerrar sesión conserva el sitio de ese ordenador; quita los que ya no uses."),
    ("Earlier messages shared with new members ({})", "Mensajes anteriores compartidos con los nuevos miembros ({})"),
    ("Edit", "Editar"),
    ("Edit message", "Editar mensaje"),
    ("Edited", "Editado"),
    ("Encrypted before this device could read this conversation.", "Cifrado antes de que este dispositivo pudiera leer esta conversación."),
    ("Encrypted end to end", "Cifrado de extremo a extremo"),
    ("Encryption failed: {}", "Falló el cifrado: {}"),
    ("Enter a current code. Your old recovery codes stop working.", "Escribe un código actual. Tus códigos de recuperación anteriores dejarán de funcionar."),
    ("Enter sends, Shift+Enter adds a line", "Intro envía, Mayús+Intro añade una línea"),
    ("Enter your password and a current code to turn two-step verification off.", "Escribe tu contraseña y un código actual para desactivar la verificación en dos pasos."),
    ("Enter your password to continue.", "Escribe tu contraseña para continuar."),
    ("Enter your password to make a new recovery key. The old one stops working.", "Escribe tu contraseña para crear una clave nueva. La anterior dejará de funcionar."),
    ("Everyone in the workspace can find it and join.", "Cualquiera del espacio de trabajo puede encontrarlo y unirse."),
    ("Everything that happens here stays on your server.", "Todo lo que pasa aquí se queda en tu servidor."),
    ("Expires after", "Caduca en"),
    ("Failed to send. Click to retry.", "No se pudo enviar. Pulsa para reintentar."),
    ("Finish in the other app", "Termina en la otra app"),
    ("Forgot your password?", "¿Has olvidado tu contraseña?"),
    ("GitHub activity", "Actividad de GitHub"),
    ("GitHub activity is visible to this workspace and the server. It is not end-to-end encrypted.", "La actividad de GitHub es visible para este espacio y el servidor. No está cifrada de extremo a extremo."),
    ("GitHub has to reach your server, so the address must be public and use https.", "GitHub tiene que llegar a tu servidor, así que la dirección debe ser pública y usar https."),
    ("GitHub in #{}", "GitHub en #{}"),
    ("Guest", "Invitado"),
    ("High contrast", "Alto contraste"),
    ("Hito in #{}", "Hito en #{}"),
    ("How many messages", "Cuántos mensajes"),
    ("I have saved it", "Ya la he guardado"),
    ("In Hito, open the project and go to its settings, then Integrations.", "En Hito, abre el proyecto y ve a sus ajustes, luego a Integraciones."),
    ("In Hito, open the project's settings, add an Ágora connection and paste this address and key. The key is shown only now.", "En Hito, abre los ajustes del proyecto, añade una conexión con Ágora y pega esta dirección y esta clave. La clave solo se muestra ahora."),
    ("Integrations", "Integraciones"),
    ("Invitation code", "Código de invitación"),
    ("Invitation code (if you have one)", "Código de invitación (si tienes uno)"),
    ("Invite people", "Invitar a gente"),
    ("Issue", "Incidencia"),
    ("It disappears for everyone in the conversation, and replies to it keep their place. This cannot be undone.", "Desaparece para todas las personas de la conversación, y sus respuestas se quedan donde están. No se puede deshacer."),
    ("Join", "Unirse"),
    ("Join a workspace", "Unirse a un espacio de trabajo"),
    ("Join call", "Unirse a la llamada"),
    ("Join channel", "Unirse al canal"),
    ("Keep me signed in on this computer", "Mantener la sesión en este ordenador"),
    ("Keep these recovery codes somewhere safe. Each one signs you in once if you lose your authenticator app. They are shown only now.", "Guarda estos códigos de recuperación en un lugar seguro. Cada uno permite iniciar sesión una vez si pierdes tu app de autenticación. Solo se muestran ahora."),
    ("Key", "Clave"),
    ("Keyboard shortcuts", "Atajos de teclado"),
    ("Language", "Idioma"),
    ("Last seen {}", "Visto por última vez el {}"),
    ("Leave call", "Salir de la llamada"),
    ("Leave channel", "Salir del canal"),
    ("Leave the content type as it is and choose Send me everything. Ágora shows only what matters.", "Deja el tipo de contenido como está y elige Send me everything. Ágora solo muestra lo que importa."),
    ("Light", "Claro"),
    ("Load older messages", "Cargar mensajes anteriores"),
    ("Loading…", "Cargando…"),
    ("Log out", "Cerrar sesión"),
    ("Log out and forget this computer", "Cerrar sesión y olvidar este equipo"),
    ("Main menu", "Menú principal"),
    ("Make a code that lets people join this workspace. Each use lets one person in, until the code runs out or expires.", "Crea un código con el que otras personas se unan a este espacio de trabajo. Cada uso deja entrar a una persona, hasta que el código se agote o caduque."),
    ("Maximize", "Maximizar"),
    ("Member", "Miembro"),
    ("Members", "Miembros"),
    ("Mentions", "Menciones"),
    ("Message #{}", "Mensaje en #{}"),
    ("Message {}", "Mensaje para {}"),
    ("Messages are encrypted for the people in a conversation when they are sent, so someone who joins later cannot read what came before. This sends a copy of the newest messages to everyone in it now.", "Los mensajes se cifran para quienes están en la conversación al enviarlos, así que quien entra después no puede leer lo anterior. Esto envía una copia de los mensajes más recientes a todos los que están ahora."),
    ("Messages are end-to-end encrypted with MLS: the server stores only ciphertext. Use TLS (https) for any server outside your computer.", "Los mensajes van cifrados de extremo a extremo con MLS: el servidor solo guarda texto cifrado. Usa TLS (https) para cualquier servidor fuera de tu ordenador."),
    ("Minimize", "Minimizar"),
    ("Monitoring", "Monitorización"),
    ("Move between channels", "Moverse entre canales"),
    ("My script", "Mi script"),
    ("Name", "Nombre"),
    ("Name it", "Ponle nombre"),
    ("Name it after the repository, for example acme/app. The name appears on every notice.", "Ponle el nombre del repositorio, por ejemplo acme/app. El nombre aparece en cada aviso."),
    ("New device in this conversation: {}", "Nuevo dispositivo en esta conversación: {}"),
    ("New direct message", "Nuevo mensaje directo"),
    ("New line", "Nueva línea"),
    ("New messages", "Mensajes nuevos"),
    ("New password", "Contraseña nueva"),
    ("New recovery codes", "Nuevos códigos de recuperación"),
    ("New recovery key", "Nueva clave de recuperación"),
    ("No GitHub activity yet, or the integration is not configured.", "Todavía no hay actividad de GitHub o la integración no está configurada."),
    ("No camera was found.", "No se encontró ninguna cámara."),
    ("No channel selected", "Ningún canal seleccionado"),
    ("No integrations in this channel yet.", "Este canal aún no tiene integraciones."),
    ("No messages yet. Say hello.", "Aún no hay mensajes. Saluda."),
    ("No microphone was found.", "No se encontró ningún micrófono."),
    ("No organizations yet.", "Todavía no hay empresas."),
    ("No replies yet", "Aún no hay respuestas"),
    ("No results.", "Sin resultados."),
    ("No speaker was found.", "No se encontró ningún altavoz."),
    ("Nobody else is here yet. Invite someone first.", "Aún no hay nadie más. Invita a alguien primero."),
    ("Nodalia controls", "Controles de Nodalia"),
    ("Not end-to-end encrypted", "Sin cifrado de extremo a extremo"),
    ("Nothing is connected to this channel yet.", "Todavía no hay nada conectado a este canal."),
    ("Notices from other apps are not end-to-end encrypted: the server can read them.", "Los avisos de otras apps no tienen cifrado de extremo a extremo: el servidor puede leerlos."),
    ("Notifications", "Notificaciones"),
    ("Notifications never include message text or sender.", "Las notificaciones nunca incluyen el texto ni el remitente."),
    ("Notify me about new messages", "Avisarme de mensajes nuevos"),
    ("Off. Add a code from an authenticator app to your password, so a stolen password is not enough to sign in.", "Desactivada. Añade a tu contraseña un código de una app de autenticación, para que una contraseña robada no baste para entrar."),
    ("Offline", "Sin conexión"),
    ("On GitHub, open the repository and go to Settings, Webhooks, then Add webhook.", "En GitHub, abre el repositorio y ve a Settings, Webhooks y Add webhook."),
    ("On. Signing in asks for a code from your authenticator app. Recovery codes left: {}.", "Activada. Al iniciar sesión se pide un código de tu app de autenticación. Códigos de recuperación restantes: {}."),
    ("Online", "En línea"),
    ("Only people with an invitation can register.", "Solo puede registrarse quien tenga invitación."),
    ("Only the owner can manage this organization; other organization roles have the same permissions.", "Solo el propietario puede administrar esta empresa; los demás roles de empresa tienen los mismos permisos."),
    ("Only the people you add can see it and read it.", "Solo las personas que añadas podrán verlo y leerlo."),
    ("Only these devices can read what is said here. The server cannot. Compare fingerprints in person to be sure.", "Solo estos dispositivos pueden leer lo que se dice aquí; el servidor no. Comparad las huellas en persona para estar seguros."),
    ("Open", "Abrir"),
    ("Open a thread", "Abrir un hilo"),
    ("Open source, self-hosted team chat.", "Chat de equipo de código abierto y alojado por ti."),
    ("Opened", "Abierto"),
    ("Or start from this command and change the text:", "O parte de este comando y cambia el texto:"),
    ("Organization members", "Miembros de la empresa"),
    ("Organization name", "Nombre de la empresa"),
    ("Organizations", "Empresas"),
    ("Other apps can post notices here: a Hito project, a build server, a script. Notices are not end-to-end encrypted; the server can read them.", "Otras apps pueden publicar avisos aquí: un proyecto de Hito, un servidor de compilación, un script. Los avisos no tienen cifrado de extremo a extremo; el servidor puede leerlos."),
    ("Other computers and sessions are signed out when you change it. Your conversations stay readable.", "Se cerrará la sesión en los demás ordenadores. Tus conversaciones seguirán siendo legibles."),
    ("Owner", "Propietario"),
    ("Password", "Contraseña"),
    ("Password and recovery", "Contraseña y recuperación"),
    ("Paste this address as the Payload URL:", "Pega esta dirección como Payload URL:"),
    ("Paste this key as the Secret:", "Pega esta clave como Secret:"),
    ("Paste this key:", "Pega esta clave:"),
    ("People", "Personas"),
    ("Pick what to connect to this channel. Each one posts its notices here.", "Elige qué conectar a este canal. Cada una publica aquí sus avisos."),
    ("Plugins", "Plugins"),
    ("Preferences", "Preferencias"),
    ("Press Add webhook. GitHub sends a first message, and Ágora answers here that it is connected.", "Pulsa Add webhook. GitHub envía un primer mensaje y Ágora avisa aquí de que está conectado."),
    ("Private", "Privado"),
    ("Private channel", "Canal privado"),
    ("Published", "Publicado"),
    ("Pull request", "Solicitud de cambios"),
    ("Pushes, pull requests, issues and releases of a repository.", "Cambios, pull requests, incidencias y versiones de un repositorio."),
    ("Quit", "Salir"),
    ("Read", "Leído"),
    ("Recovery code", "Código de recuperación"),
    ("Recovery key or code", "Clave de recuperación o código"),
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
    ("Reset your password", "Restablece tu contraseña"),
    ("Restore", "Restaurar"),
    ("Review", "Revisión"),
    ("Save", "Guardar"),
    ("Save file", "Guardar archivo"),
    ("Saved {}.", "Se ha guardado {}."),
    ("Scan this code with an authenticator app (any app that supports TOTP), or type the key. Then enter the six digits it shows.", "Escanea este código con una app de autenticación (cualquiera compatible con TOTP) o escribe la clave. Después introduce los seis dígitos que muestre."),
    ("Script", "Script"),
    ("Search", "Buscar"),
    ("Search messages", "Buscar mensajes"),
    ("Search results", "Resultados de la búsqueda"),
    ("Security alert", "Alerta de seguridad"),
    ("Security and devices", "Seguridad y dispositivos"),
    ("Send", "Enviar"),
    ("Send a POST to this address with a JSON body whose text field holds the notice. Sign it with the key: the header X-Agora-Signature is sha256= and the HMAC-SHA256 of the body in hex. The key is shown only now.", "Envía un POST a esta dirección con un cuerpo JSON cuyo campo text contenga el aviso. Fírmalo con la clave: la cabecera X-Agora-Signature es sha256= y el HMAC-SHA256 del cuerpo en hexadecimal. La clave solo se muestra ahora."),
    ("Send a message", "Enviar un mensaje"),
    ("Send a test", "Enviar una prueba"),
    ("Send a test notice to this channel to see it arrive.", "Envía un aviso de prueba a este canal para verlo llegar."),
    ("Send an HTTP POST with a JSON body whose text field holds the notice, to this address:", "Envía un HTTP POST con un cuerpo JSON cuyo campo text contenga el aviso, a esta dirección:"),
    ("Send files", "Enviar archivos"),
    ("Sending…", "Enviando…"),
    ("Sent", "Enviado"),
    ("Sent · delivered to {} of {}", "Enviado · entregado a {} de {}"),
    ("Sent. Look for it in the channel.", "Enviado. Búscalo en el canal."),
    ("Server", "Servidor"),
    ("Set up", "Configurar"),
    ("Set up a web connection", "Configurar una conexión web"),
    ("Share", "Compartir"),
    ("Share earlier messages", "Compartir mensajes anteriores"),
    ("Short and lower case, like design or launch-plan.", "Corto y en minúsculas, como diseño o plan-de-lanzamiento."),
    ("Show or hide", "Mostrar u ocultar"),
    ("Show or hide the details panel", "Mostrar u ocultar el panel de detalles"),
    ("Show or hide the side panel", "Mostrar u ocultar el panel lateral"),
    ("Shown once. Share it privately: whoever has it can join.", "Se muestra una vez. Compártelo en privado: quien lo tenga puede entrar."),
    ("Sign in", "Iniciar sesión"),
    ("Sign in to Ágora", "Inicia sesión en Ágora"),
    ("Sign the body with this key. Send the HMAC-SHA256 in hex as sha256=… in the X-Agora-Signature header:", "Firma el cuerpo con esta clave. Envía el HMAC-SHA256 en hexadecimal como sha256=… en la cabecera X-Agora-Signature:"),
    ("Signed in", "Con sesión iniciada"),
    ("Signed out", "Sin sesión"),
    ("Signed out. Sign in again.", "Sesión cerrada. Vuelve a iniciar sesión."),
    ("Someone", "Alguien"),
    ("Start a call", "Iniciar una llamada"),
    ("Step {} of 3 · {}", "Paso {} de 3 · {}"),
    ("System", "Sistema"),
    ("System controls", "Controles del sistema"),
    ("Tasks and milestones of a Hito project show up here as they are completed or change.", "Las tareas e hitos de un proyecto de Hito aparecen aquí cuando se completan o cambian."),
    ("Tasks and milestones of a project, as they change.", "Tareas e hitos de un proyecto, según cambian."),
    ("Test notice sent. Look for it in the channel.", "Aviso de prueba enviado. Búscalo en el canal."),
    ("That already exists.", "Eso ya existe."),
    ("That code is not valid. Check the time on this computer and try again.", "Ese código no es válido. Comprueba la hora de este ordenador y vuelve a intentarlo."),
    ("That is not valid.", "Eso no es válido."),
    ("That no longer exists, or you cannot see it.", "Eso ya no existe, o no puedes verlo."),
    ("The camera cannot be used: {}", "No se puede usar la cámara: {}"),
    ("The details panel", "El panel de detalles"),
    ("The key is shown only now. If you lose it, remove the connection and make a new one.", "La clave solo se muestra ahora. Si la pierdes, elimina la conexión y crea otra."),
    ("The microphone cannot be used: {}", "No se puede usar el micrófono: {}"),
    ("The name appears on every notice, so people know where it came from.", "El nombre aparece en cada aviso, para que se sepa de dónde viene."),
    ("The password is not right.", "La contraseña no es correcta."),
    ("The server address is not valid.", "La dirección del servidor no es válida."),
    ("The server answered something unexpected. Is it running a compatible version of Ágora?", "El servidor respondió algo inesperado. ¿Ejecuta una versión compatible de Ágora?"),
    ("The speaker cannot be used: {}", "No se puede usar el altavoz: {}"),
    ("The system notification service is unavailable.", "El servicio de notificaciones del sistema no está disponible."),
    ("The window buttons on Linux.", "Los botones de la ventana en Linux."),
    ("Theme", "Tema"),
    ("There are no messages to share yet.", "Aún no hay mensajes que compartir."),
    ("This account uses two-step verification.", "Esta cuenta usa verificación en dos pasos."),
    ("This address uses http: passwords and messages travel unencrypted. Use it only on your own computer or network.", "Esta dirección usa http: las contraseñas y los mensajes viajan sin cifrar. Úsala solo en tu ordenador o tu red."),
    ("This call is full.", "Esta llamada está llena."),
    ("This computer", "Este ordenador"),
    ("This is the start of #{}.", "Este es el principio de #{}."),
    ("This is the start of your conversation with {}.", "Este es el principio de tu conversación con {}."),
    ("This is your recovery key. If you forget your password, choose “Forgot your password?” on the sign-in screen and enter it. Keep it somewhere safe, apart from your password. It is shown only now.", "Esta es tu clave de recuperación. Si olvidas tu contraseña, elige «¿Has olvidado tu contraseña?» en la pantalla de inicio de sesión e introdúcela. Guárdala en un lugar seguro, aparte de tu contraseña. Solo se muestra ahora."),
    ("This server does not accept new accounts.", "Este servidor no admite cuentas nuevas."),
    ("Thread", "Hilo"),
    ("Too many attempts. Wait a minute.", "Demasiados intentos. Espera un minuto."),
    ("Too many requests. Wait a moment and try again.", "Demasiadas peticiones. Espera un momento e inténtalo de nuevo."),
    ("Topic", "Tema del canal"),
    ("Topic (optional)", "Tema (opcional)"),
    ("Turn off", "Desactivar"),
    ("Turn on", "Activar"),
    ("Turn the camera off", "Apagar la cámara"),
    ("Turn the camera on", "Encender la cámara"),
    ("Turn the microphone off", "Silenciar el micrófono"),
    ("Turn the microphone on", "Activar el micrófono"),
    ("Two-step verification", "Verificación en dos pasos"),
    ("Type {} to confirm", "Escribe {} para confirmar"),
    ("Use a recovery code", "Usar un código de recuperación"),
    ("Use letters, numbers and hyphens, up to 80 characters.", "Usa letras, números y guiones, hasta 80 caracteres."),
    ("Use the authenticator app", "Usar la app de autenticación"),
    ("Use the recovery key you saved, or a code from whoever runs this server.", "Usa la clave de recuperación que guardaste, o un código de quien administra este servidor."),
    ("Username", "Usuario"),
    ("Uses", "Usos"),
    ("Version {}", "Versión {}"),
    ("View", "Ver"),
    ("Webhook", "Webhook"),
    ("Website launch", "Lanzamiento de la web"),
    ("What do you want to connect?", "¿Qué quieres conectar?"),
    ("What to connect", "Qué conectar"),
    ("Who can see it", "Quién puede verlo"),
    ("Window", "Ventana"),
    ("Working…", "Procesando…"),
    ("Workspace", "Espacio de trabajo"),
    ("Workspace admins can add integrations.", "Los administradores del espacio de trabajo pueden añadir integraciones."),
    ("Workspace admins can connect apps.", "Los administradores del espacio de trabajo pueden conectar apps."),
    ("Workspace name", "Nombre del espacio de trabajo"),
    ("Workspaces", "Espacios de trabajo"),
    ("You are not a member of this channel.", "No eres miembro de este canal."),
    ("You are not allowed to do that.", "No tienes permiso para hacer eso."),
    ("You can't join this call.", "No puedes unirte a esta llamada."),
    ("You don't belong to any workspace yet.", "Aún no perteneces a ningún espacio de trabajo."),
    ("You have a new message.", "Tienes un mensaje nuevo."),
    ("You have a recovery key. If you forget your password, it lets you choose a new one from the sign-in screen.", "Tienes una clave de recuperación. Si olvidas tu contraseña, te permite elegir una nueva desde la pantalla de inicio de sesión."),
    ("You have no recovery key yet. Without one, only whoever runs this server can help you if you forget your password.", "Aún no tienes clave de recuperación. Sin ella, si olvidas tu contraseña solo puede ayudarte quien administra este servidor."),
    ("You joined this call from another device.", "Te has unido a esta llamada desde otro dispositivo."),
    ("Your devices", "Tus dispositivos"),
    ("Your session ended. Sign in again.", "Tu sesión ha terminado. Vuelve a iniciar sesión."),
    ("added {}", "añadida el {}"),
    ("authentication failed", "No se pudo comprobar tu identidad. Revisa la contraseña."),
    ("deleted message", "mensaje eliminado"),
    ("direct conversations do not take integrations", "Las conversaciones directas no admiten integraciones."),
    ("enter the code from your authenticator app", "Escribe el código de tu app de autenticación."),
    ("everyone in the workspace", "todo el espacio de trabajo"),
    ("invited members only", "solo los miembros invitados"),
    ("last notice {}", "último aviso {}"),
    ("no notices yet", "aún sin avisos"),
    ("only admins and the channel's creator can change it", "Solo los administradores y quien creó el canal pueden cambiarlo."),
    ("only owners and admins can invite people", "Solo el propietario y los administradores pueden invitar a personas."),
    ("only the workspace owner can delete it", "Solo quien creó el espacio de trabajo puede eliminarlo."),
    ("only workspace admins can manage integrations", "Solo los administradores del espacio de trabajo pueden gestionar integraciones."),
    ("only workspace owners and admins can change the icon", "Solo el propietario y los administradores pueden cambiar el icono."),
    ("that code is not valid", "Ese código no es válido."),
    ("that recovery code is not valid or has expired", "Esa clave o código de recuperación no es válido o ha caducado."),
    ("that username is taken", "Ese nombre de usuario ya está en uso."),
    ("type the workspace's name exactly to confirm deleting it", "Escribe el nombre exacto del espacio de trabajo para confirmar que quieres eliminarlo."),
    ("wrong username or password", "Usuario o contraseña incorrectos."),
    ("{} (this device)", "{} (este dispositivo)"),
    ("{} and everything in it will be deleted for good: every channel, message, file, task and integration, for all {} of its members. This cannot be undone.", "{} y todo lo que contiene se eliminará para siempre: todos los canales, mensajes, archivos, tareas e integraciones, para sus {} miembros. No se puede deshacer."),
    ("{} is ready", "{} está lista"),
    ("{} is too large to send.", "{} es demasiado grande para enviarlo."),
    ("{} is typing…", "{} está escribiendo…"),
    ("{} members", "{} miembros"),
    ("{} people are typing…", "{} personas están escribiendo…"),
    ("{} people in the call", "{} personas en la llamada"),
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
