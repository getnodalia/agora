//! The Linux desktop's global menu: Ágora's commands exported over D-Bus as a
//! `com.canonical.dbusmenu` object and registered with the desktop's
//! `com.canonical.AppMenu.Registrar`, so a top bar that shows the focused app's menus (Nodalix's)
//! shows Ágora's. The hamburger menu in the window stays: it is where NDL keeps Preferences,
//! Keyboard shortcuts and About on every desktop.
//!
//! The registration names this process rather than a window (a Wayland window has no global id),
//! so only a registrar that pairs a menu with its window by process can show it: Nodalix's, whose
//! process also owns `space.unmade.GlobalMenu`. A chosen item reaches the app as a command id,
//! which [`crate::app::App`] turns into the same action its button would send.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, StructureBuilder, Value};

const REGISTRAR: &str = "com.canonical.AppMenu.Registrar";
const REGISTRAR_PATH: &str = "/com/canonical/AppMenu/Registrar";
const NODALIX_MENU: &str = "space.unmade.GlobalMenu";
const MENU_PATH: &str = "/com/getnodalia/GlobalMenu";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

/// One command in the global menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What the app runs when it is chosen (`app.preferences`).
    pub id: &'static str,
    /// The top-level menu it is in, already translated.
    pub menu: String,
    pub label: String,
    pub enabled: bool,
}

/// One row of the exported menu.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Item {
        id: &'static str,
        label: String,
        enabled: bool,
    },
    Separator,
    Submenu {
        label: String,
        children: Vec<i32>,
    },
}

/// The menus as the desktop reads them: rows by id (0 is the root, whose children are the
/// top-level menus).
#[derive(Default)]
struct Shared {
    rows: HashMap<i32, Row>,
    root: Vec<i32>,
    revision: u32,
}

impl Shared {
    /// Number the rows from 1: each menu in the order it first appears, then its items. An item
    /// whose id is empty is a separator. The same list always gets the same ids.
    fn rows_of(items: &[Item]) -> (HashMap<i32, Row>, Vec<i32>) {
        let mut menus: Vec<&str> = Vec::new();
        for item in items {
            if !menus.contains(&item.menu.as_str()) {
                menus.push(&item.menu);
            }
        }
        let (mut rows, mut next, mut root) = (HashMap::new(), 1i32, Vec::new());
        for menu in menus {
            let id = next;
            next = next.saturating_add(1);
            let mut children = Vec::new();
            for item in items.iter().filter(|i| i.menu == menu) {
                let row = if item.id.is_empty() {
                    Row::Separator
                } else {
                    Row::Item {
                        id: item.id,
                        label: item.label.clone(),
                        enabled: item.enabled,
                    }
                };
                rows.insert(next, row);
                children.push(next);
                next = next.saturating_add(1);
            }
            rows.insert(
                id,
                Row::Submenu {
                    label: menu.to_owned(),
                    children,
                },
            );
            root.push(id);
        }
        (rows, root)
    }

    fn children(&self, id: i32) -> &[i32] {
        match self.rows.get(&id) {
            _ if id == 0 => &self.root,
            Some(Row::Submenu { children, .. }) => children,
            _ => &[],
        }
    }

    /// `id`'s dbusmenu properties.
    fn properties(&self, id: i32) -> HashMap<String, OwnedValue> {
        let mut p = HashMap::new();
        let mut put = |k: &str, v: Value<'_>| {
            if let Ok(v) = v.try_to_owned() {
                p.insert(k.to_string(), v);
            }
        };
        match self.rows.get(&id) {
            None if id == 0 => put("children-display", Value::from("submenu")),
            None => {}
            Some(Row::Submenu { label, .. }) => {
                put("label", Value::from(mnemonic_free(label)));
                put("children-display", Value::from("submenu"));
            }
            Some(Row::Separator) => put("type", Value::from("separator")),
            Some(Row::Item { label, enabled, .. }) => {
                put("label", Value::from(mnemonic_free(label)));
                put("enabled", Value::from(*enabled));
            }
        }
        p
    }

    /// `id` and `depth` levels of its children (`-1`: all), the reply to `GetLayout`.
    fn layout_item(&self, id: i32, depth: i32) -> LayoutItem {
        let children = if depth == 0 {
            Vec::new()
        } else {
            self.children(id)
                .iter()
                .filter_map(|&c| self.layout(c, depth.saturating_sub(1)).try_to_owned().ok())
                .collect()
        };
        LayoutItem {
            id,
            properties: self.properties(id),
            children,
        }
    }

    /// [`Shared::layout_item`] as a variant's value: how dbusmenu nests the children.
    fn layout(&self, id: i32, depth: i32) -> Value<'static> {
        let children: Vec<Value<'static>> = if depth == 0 {
            Vec::new()
        } else {
            self.children(id)
                .iter()
                .map(|&c| self.layout(c, depth.saturating_sub(1)))
                .collect()
        };
        let children = zbus::zvariant::Array::from(children);
        let built = StructureBuilder::new()
            .add_field(id)
            .add_field(self.properties(id))
            .append_field(Value::Array(children))
            .build();
        built
            .map(Value::Structure)
            .unwrap_or_else(|_| Value::from(0))
    }
}

/// A label with its underscores doubled: dbusmenu reads a single one as a mnemonic.
fn mnemonic_free(label: &str) -> String {
    label.replace('_', "__")
}

/// One item of the menu tree as dbusmenu sends it: `(ia{sv}av)`, each child a variant holding the
/// same structure.
#[derive(Debug, serde::Serialize, zbus::zvariant::Type)]
struct LayoutItem {
    id: i32,
    properties: HashMap<String, OwnedValue>,
    children: Vec<OwnedValue>,
}

/// The `com.canonical.dbusmenu` object.
struct MenuObject {
    shared: Arc<Mutex<Shared>>,
    commands: Mutex<Sender<&'static str>>,
    ctx: egui::Context,
}

impl MenuObject {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// An enabled item's command goes to the app; anything else (a hover, a submenu, a disabled
    /// item) does nothing.
    fn chosen(&self, id: i32, kind: &str) {
        if kind != "clicked" {
            return;
        }
        let command = match self.lock().rows.get(&id) {
            Some(Row::Item {
                id, enabled: true, ..
            }) => *id,
            _ => return,
        };
        // The app gone means nothing is left to run it.
        let _ = self
            .commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(command);
        self.ctx.request_repaint();
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl MenuObject {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> String {
        "ltr".into()
    }
    #[zbus(property)]
    fn status(&self) -> String {
        "normal".into()
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> (u32, LayoutItem) {
        let s = self.lock();
        (s.revision, s.layout_item(parent_id, recursion_depth))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let s = self.lock();
        // An empty list asks for every item (the dbusmenu contract).
        let ids = if ids.is_empty() {
            s.rows.keys().copied().collect()
        } else {
            ids
        };
        ids.into_iter()
            .filter(|id| s.rows.contains_key(id))
            .map(|id| (id, s.properties(id)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> OwnedValue {
        self.lock()
            .properties(id)
            .remove(&name)
            .unwrap_or_else(|| OwnedValue::from(0i32))
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        self.chosen(id, &event_id);
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, kind, _, _) in events {
            self.chosen(id, &kind);
        }
        Vec::new()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let s = self.lock();
        let unknown = ids
            .into_iter()
            .filter(|id| *id != 0 && !s.rows.contains_key(id))
            .collect();
        (Vec::new(), unknown)
    }
}

/// Whether the registrar on the bus is Nodalix's, the one registrar that finds a window's menu by
/// its process.
fn nodalix_registrar(conn: &Connection) -> bool {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
        return false;
    };
    let owner = |name: &str| {
        zbus::names::BusName::try_from(name)
            .ok()
            .and_then(|n| dbus.get_name_owner(n).ok())
            .map(|o| o.to_string())
    };
    matches!((owner(REGISTRAR), owner(NODALIX_MENU)), (Some(r), Some(n)) if r == n)
}

/// Hand the menu to the registrar.
fn register(conn: &Connection) {
    let path = zbus::zvariant::ObjectPath::from_static_str_unchecked(MENU_PATH);
    let accepted = nodalix_registrar(conn)
        && conn
            .call_method(
                Some(REGISTRAR),
                REGISTRAR_PATH,
                Some(REGISTRAR),
                "RegisterWindow",
                &(std::process::id(), path),
            )
            .is_ok();
    tracing::info!(
        "global menu: {}",
        if accepted {
            "shown in the desktop's top bar"
        } else {
            "no registrar that shows it"
        }
    );
}

/// What the app keeps of the global menu.
pub struct GlobalMenu {
    /// The chosen items' command ids.
    pub commands: Receiver<&'static str>,
    conn: Connection,
    shared: Arc<Mutex<Shared>>,
}

impl GlobalMenu {
    /// Take a changed menu to the desktop. Unchanged menus cost a comparison.
    pub fn update(&self, items: &[Item]) {
        let (rows, root) = Shared::rows_of(items);
        let revision = {
            let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            if s.rows == rows && s.root == root {
                return;
            }
            s.rows = rows;
            s.root = root;
            s.revision = s.revision.wrapping_add(1);
            s.revision
        };
        // A failure leaves the desktop on the last menu it read.
        let _ = self.conn.emit_signal(
            None::<&str>,
            MENU_PATH,
            MENU_INTERFACE,
            "LayoutUpdated",
            &(revision, 0i32),
        );
    }
}

/// Export the menu and register it with the desktop's registrar, now and whenever a registrar
/// appears. `None` without a session bus.
pub fn install(ctx: &egui::Context, items: &[Item]) -> Option<GlobalMenu> {
    let (rows, root) = Shared::rows_of(items);
    let shared = Arc::new(Mutex::new(Shared {
        rows,
        root,
        revision: 1,
    }));
    let (tx, commands) = channel();
    let object = MenuObject {
        shared: shared.clone(),
        commands: Mutex::new(tx),
        ctx: ctx.clone(),
    };
    let conn = match zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(MENU_PATH, object))
        .and_then(|b| b.build())
    {
        Ok(conn) => conn,
        Err(e) => {
            tracing::info!("global menu: no session bus ({e})");
            return None;
        }
    };
    register(&conn);
    // The registrar may start after Ágora, or restart with the shell: register again whenever it
    // appears.
    let watch = conn.clone();
    let spawned = std::thread::Builder::new()
        .name("global-menu".into())
        .spawn(move || {
            let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&watch) else {
                return;
            };
            let Ok(changes) = dbus.receive_name_owner_changed() else {
                return;
            };
            for change in changes {
                let Ok(args) = change.args() else { continue };
                let name = args.name().as_str();
                if (name == REGISTRAR || name == NODALIX_MENU) && args.new_owner().is_some() {
                    register(&watch);
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("global menu: cannot watch the registrar ({e})");
    }
    Some(GlobalMenu {
        commands,
        conn,
        shared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &'static str, menu: &str, label: &str, enabled: bool) -> Item {
        Item {
            id,
            menu: menu.into(),
            label: label.into(),
            enabled,
        }
    }

    fn items() -> Vec<Item> {
        vec![
            item("app.preferences", "Ágora", "Preferences", true),
            item("", "Ágora", "", true),
            item("app.logout", "Ágora", "Log_out", true),
            item("workspace.invite", "Workspace", "Invite people", false),
        ]
    }

    #[test]
    fn menus_keep_their_order_and_items() {
        let (rows, root) = Shared::rows_of(&items());
        assert_eq!(root, vec![1, 5]);
        let s = Shared {
            rows,
            root,
            ..Default::default()
        };
        let text = |id: i32, key: &str| {
            s.properties(id)
                .get(key)
                .and_then(|v| <&str>::try_from(v).ok().map(str::to_string))
        };
        assert_eq!(text(1, "label").as_deref(), Some("Ágora"));
        assert_eq!(s.children(1), &[2, 3, 4]);
        assert_eq!(text(3, "type").as_deref(), Some("separator"));
        assert_eq!(text(4, "label").as_deref(), Some("Log__out"));
        use zbus::zvariant::Type as _;
        assert_eq!(LayoutItem::SIGNATURE.to_string(), "(ia{sv}av)");
        assert_eq!(s.layout_item(0, -1).children.len(), 2);
        assert!(s.layout_item(0, 0).children.is_empty());
    }

    #[test]
    fn a_click_sends_the_command_and_a_disabled_item_nothing() {
        let (rows, root) = Shared::rows_of(&items());
        let (tx, rx) = channel();
        let object = MenuObject {
            shared: Arc::new(Mutex::new(Shared {
                rows,
                root,
                ..Default::default()
            })),
            commands: Mutex::new(tx),
            ctx: egui::Context::default(),
        };
        object.chosen(2, "clicked");
        object.chosen(3, "clicked");
        object.chosen(6, "clicked");
        object.chosen(2, "hovered");
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec!["app.preferences"]);
    }
}
