//! wasm-bindgen wrapper around [`remember_core::Session`] for the browser
//! build. Glue only: commands arrive as plain JS objects shaped like
//! [`Command`] (`{ type: "Add", title, ... }`), snapshots leave the same
//! way, and persistence is the host page's job — it saves the bytes from
//! [`Remember::export`] wherever it likes and hands them back to the
//! constructor next time.

use remember_core::{fresh_peer_id, Clock, Command, Doc, Session, UuidSource, ViewFilter};
use wasm_bindgen::prelude::*;

/// `SystemClock` reads `std::time::SystemTime`, which panics on
/// wasm32-unknown-unknown — ask the browser instead.
struct JsClock;

impl Clock for JsClock {
    fn now(&self) -> i64 {
        (js_sys::Date::now() / 1000.0) as i64
    }
}

fn js_err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

#[wasm_bindgen]
pub struct Remember {
    session: Session,
}

#[wasm_bindgen]
impl Remember {
    /// `saved` is a previous [`Remember::export`], or `undefined` for a
    /// fresh document. Each page load gets its own Loro peer id, so two
    /// tabs never mint conflicting op ids.
    #[wasm_bindgen(constructor)]
    pub fn new(saved: Option<Vec<u8>>, current_list: Option<String>) -> Result<Remember, JsError> {
        let peer_id = fresh_peer_id();
        let doc = match saved {
            Some(bytes) => Doc::load(peer_id, &bytes),
            None => Doc::new(peer_id),
        }
        .map_err(js_err)?;
        Ok(Self {
            session: Session::new(doc, current_list),
        })
    }

    pub fn dispatch(&mut self, command: JsValue) -> Result<(), JsError> {
        let command: Command = serde_wasm_bindgen::from_value(command).map_err(js_err)?;
        self.session
            .dispatch(command, &JsClock, &UuidSource)
            .map_err(js_err)?;
        Ok(())
    }

    /// `"All"`, `"Active"` or `"Completed"`.
    pub fn set_view(&mut self, view: JsValue) -> Result<(), JsError> {
        let view: ViewFilter = serde_wasm_bindgen::from_value(view).map_err(js_err)?;
        self.session.set_view(view);
        Ok(())
    }

    pub fn set_current_list(&mut self, list_id: String) {
        self.session.set_current_list(list_id);
    }

    pub fn current_list(&self) -> String {
        self.session.current_list().to_string()
    }

    pub fn set_local_offset_seconds(&mut self, offset_seconds: i32) {
        self.session.set_local_offset_seconds(offset_seconds);
    }

    /// The current `Snapshot`, as a plain JS object.
    pub fn snapshot(&self) -> Result<JsValue, JsError> {
        serde_wasm_bindgen::to_value(&self.session.current(&JsClock)).map_err(js_err)
    }

    /// A `DueDetection` object, or `undefined`.
    pub fn detect_due(&self, text: &str) -> Result<JsValue, JsError> {
        serde_wasm_bindgen::to_value(&self.session.detect_due(text, &JsClock)).map_err(js_err)
    }

    /// The whole document as Loro snapshot bytes, for the page to persist.
    pub fn export(&self) -> Result<Vec<u8>, JsError> {
        self.session.doc().export_snapshot().map_err(js_err)
    }
}
