use crate::gui::Action;
use ksni::{
    blocking::{Handle, TrayMethods},
    menu::StandardItem,
    MenuItem,
};

pub struct Tray {
    pub tx: async_channel::Sender<Action>,
}
impl Tray {
    fn item(&self, label: impl Into<String>, action: Action, enabled: bool) -> MenuItem<Self> {
        let tx = self.tx.clone();
        StandardItem {
            label: label.into(),
            enabled,
            activate: Box::new(move |_| {
                let _ = tx.try_send(action.clone());
            }),
            ..Default::default()
        }
        .into()
    }
}
impl ksni::Tray for Tray {
    const MENU_ON_ACTIVATE: bool = false;
    fn id(&self) -> String {
        "monitor-control".into()
    }
    fn title(&self) -> String {
        "Monitor Control".into()
    }
    fn icon_name(&self) -> String {
        "display-brightness-symbolic".into()
    }
    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }
    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.tx.try_send(Action::Show);
    }
    fn secondary_activate(&mut self, _: i32, _: i32) {
        let _ = self.tx.try_send(Action::Show);
    }
    fn scroll(&mut self, delta: i32, _: ksni::Orientation) {
        if delta != 0 {
            let _ = self.tx.try_send(Action::Adjust(delta.signum()));
        }
    }
    fn watcher_online(&self) {
        let _ = self.tx.try_send(Action::TrayAvailable(true));
    }
    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        let _ = self.tx.try_send(Action::TrayAvailable(false));
        true
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            self.item("Open Monitor Control", Action::Show, true),
            MenuItem::Separator,
            self.item("Quit", Action::Quit, true),
        ]
    }
}
pub fn start(tx: async_channel::Sender<Action>) -> Option<Handle<Tray>> {
    match (Tray { tx: tx.clone() }).assume_sni_available(true).spawn() {
        Ok(handle) => Some(handle),
        Err(e) => {
            let _ = tx.try_send(Action::Notice(format!("Tray connection unavailable: {e}")));
            None
        }
    }
}
