//! Accessibility for Snap It's custom-drawn windows: keyboard focus and UI Automation.
//!
//! Each window describes its interactive parts as a list of [`Node`]s while it renders and
//! hands the list to [`Access::publish`]. That list is
//! - the **Tab order** for keyboard navigation ([`Access::tab`]), and
//! - the **UI Automation tree** that Narrator and other screen readers read: the window's
//!   `WM_GETOBJECT` (answered in `ui::proc`) returns a [`Root`] provider whose children are
//!   the nodes, with the Invoke / Toggle / Value / SelectionItem patterns their roles need.
//!
//! Providers only read a snapshot kept in a global registry, so UI Automation may call them
//! from any thread. Anything a screen reader asks for (press, toggle, select, focus, set a
//! value) is **posted** to the window as [`WM_SNAPIT_A11Y`] and reaches the handler through
//! [`super::Handler::a11y_action`] once the message loop runs it: never re-entrantly, like
//! every other deferred action in Snap It.

use std::sync::Mutex;

use snapit_gfx::Rect;
use windows::core::{implement, Error, Interface, Result, BSTR, BOOL, PCWSTR};
use windows::Win32::Foundation::{E_ACCESSDENIED, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_I4};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to a window: `wparam` is the node key, `lparam` a `Box<Action>`.
pub const WM_SNAPIT_A11Y: u32 = WM_APP + 95;

/// Virtual-key codes as plain `u32`s (`WM_KEYDOWN`'s `wparam`), usable in `match` patterns.
pub mod vk {
    pub const VK_TAB: u32 = 0x09;
    pub const VK_RETURN: u32 = 0x0D;
    pub const VK_ESCAPE: u32 = 0x1B;
    pub const VK_SPACE: u32 = 0x20;
    pub const VK_PRIOR: u32 = 0x21;
    pub const VK_NEXT: u32 = 0x22;
    pub const VK_END: u32 = 0x23;
    pub const VK_HOME: u32 = 0x24;
    pub const VK_LEFT: u32 = 0x25;
    pub const VK_UP: u32 = 0x26;
    pub const VK_RIGHT: u32 = 0x27;
    pub const VK_DOWN: u32 = 0x28;
    pub const VK_DELETE: u32 = 0x2E;
}

/// What a node is, which decides its UI Automation control type and patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Push button (Invoke).
    Button,
    /// On/off switch (Toggle).
    CheckBox,
    /// Editable text (Value).
    Edit,
    /// A box that records a key combination when focused (Value, read-only).
    Hotkey,
    /// One choice of a segmented control (SelectionItem).
    RadioButton,
    /// A page in a sidebar or tab strip (SelectionItem).
    TabItem,
    /// An entry in a list or grid (SelectionItem + Invoke to open).
    ListItem,
    /// Static text read by screen readers; never focusable.
    Text,
}

/// One accessible part of a window, in client coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Stable id within the window (the window maps it back to its own hit target).
    pub key: u32,
    pub rect: Rect,
    pub role: Role,
    pub name: String,
    /// Longer description (UI Automation HelpText).
    pub help: Option<String>,
    /// Text of an Edit / Hotkey.
    pub value: Option<String>,
    /// CheckBox state, or whether a RadioButton / TabItem / ListItem is selected.
    pub checked: Option<bool>,
    pub enabled: bool,
    /// Scrolled out of view (still reachable with Tab, which scrolls it in).
    pub offscreen: bool,
    /// Part of the Tab order.
    pub tab_stop: bool,
    /// Announced when its name changes (a step heading, a status message).
    pub live: bool,
}

impl Node {
    pub fn new(key: u32, rect: Rect, role: Role, name: impl Into<String>) -> Node {
        Node {
            key,
            rect,
            role,
            name: name.into(),
            help: None,
            value: None,
            checked: None,
            enabled: true,
            offscreen: false,
            tab_stop: role != Role::Text,
            live: false,
        }
    }
    pub fn help(mut self, h: impl Into<String>) -> Node {
        let h = h.into();
        self.help = (!h.is_empty()).then_some(h);
        self
    }
    pub fn value(mut self, v: impl Into<String>) -> Node {
        self.value = Some(v.into());
        self
    }
    pub fn checked(mut self, on: bool) -> Node {
        self.checked = Some(on);
        self
    }
    pub fn enabled(mut self, on: bool) -> Node {
        self.enabled = on;
        self
    }
    pub fn live(mut self) -> Node {
        self.live = true;
        self
    }
    pub fn no_tab(mut self) -> Node {
        self.tab_stop = false;
        self
    }
}

/// A request from a screen reader (or other UI Automation client).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Invoke,
    Toggle,
    Select,
    Focus,
    SetValue(String),
}

/// Keyboard focus and the accessible tree of one window.
#[derive(Default)]
pub struct Access {
    /// Rebuilt every render, in Tab order.
    pub nodes: Vec<Node>,
    /// Key of the focused node.
    pub focus: Option<u32>,
    /// Draw the focus ring (focus moved by keyboard, not by mouse).
    pub ring: bool,
}

impl Access {
    /// Starts a new frame's node list.
    pub fn begin(&mut self) {
        self.nodes.clear();
    }

    pub fn push(&mut self, n: Node) {
        self.nodes.push(n);
    }

    pub fn get(&self, key: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.key == key)
    }

    /// Focus moved by the mouse (or programmatically): no focus ring.
    pub fn set_focus(&mut self, key: Option<u32>) {
        self.focus = key;
        self.ring = false;
    }

    /// The next (or previous) Tab stop after the focused node, wrapping around. Works from
    /// the focused node's place in the order even if it isn't a Tab stop itself.
    pub fn next(&self, back: bool) -> Option<u32> {
        let n = self.nodes.len();
        let is_stop = |i: usize| self.nodes[i].tab_stop && self.nodes[i].enabled;
        let cur = self.focus.and_then(|k| self.nodes.iter().position(|x| x.key == k));
        (1..=n)
            .map(|step| match (cur, back) {
                (None, false) => step - 1,
                (None, true) => n - step,
                (Some(c), false) => (c + step) % n,
                (Some(c), true) => (c + n - step % n) % n,
            })
            .find(|&i| is_stop(i))
            .map(|i| self.nodes[i].key)
    }

    /// Rect of the focused node, if the ring should be drawn.
    pub fn ring_rect(&self) -> Option<Rect> {
        if !self.ring {
            return None;
        }
        self.focus.and_then(|k| self.get(k)).filter(|n| !n.offscreen).map(|n| n.rect)
    }

    /// Hands the frame's nodes to UI Automation and raises focus / state change events.
    pub fn publish(&self, hwnd: HWND) {
        publish(hwnd, self.nodes.clone(), self.focus);
    }
}

// ---------- registry ----------

struct Tree {
    hwnd: isize,
    nodes: Vec<Node>,
    focus: Option<u32>,
}

static TREES: Mutex<Vec<Tree>> = Mutex::new(Vec::new());

fn with_tree<R>(hwnd: isize, f: impl FnOnce(&Tree) -> R) -> Option<R> {
    let trees = TREES.lock().ok()?;
    trees.iter().find(|t| t.hwnd == hwnd).map(f)
}

fn node(hwnd: isize, key: u32) -> Result<Node> {
    with_tree(hwnd, |t| t.nodes.iter().find(|n| n.key == key).cloned()).flatten().ok_or_else(not_available)
}

fn not_available() -> Error {
    hr(UIA_E_ELEMENTNOTAVAILABLE)
}

/// The UIA_E_* codes are plain `u32`s.
fn hr(code: u32) -> Error {
    Error::from(windows::core::HRESULT(code as i32))
}

/// What `hwnd` last published: its nodes and focused key (for `--selftest`).
pub fn published(hwnd: HWND) -> (Vec<Node>, Option<u32>) {
    with_tree(hwnd.0 as isize, |t| (t.nodes.clone(), t.focus)).unwrap_or_default()
}

/// Whether `hwnd` publishes an accessible tree (so `WM_GETOBJECT` should be answered).
pub fn registered(hwnd: HWND) -> bool {
    with_tree(hwnd.0 as isize, |_| ()).is_some()
}

enum Event {
    Focus(u32),
    Toggle(u32, bool, bool),
    Selected(u32, bool, bool),
    Value(u32, String, String),
    Live(u32),
}

fn publish(hwnd: HWND, nodes: Vec<Node>, focus: Option<u32>) {
    let h = hwnd.0 as isize;
    let mut events = Vec::new();
    {
        let Ok(mut trees) = TREES.lock() else { return };
        let i = match trees.iter().position(|t| t.hwnd == h) {
            Some(i) => i,
            None => {
                trees.push(Tree { hwnd: h, nodes: Vec::new(), focus: None });
                trees.len() - 1
            }
        };
        let old = std::mem::replace(&mut trees[i], Tree { hwnd: h, nodes, focus });
        let new = &trees[i];
        if new.focus != old.focus {
            if let Some(k) = new.focus {
                events.push(Event::Focus(k));
            }
        }
        for n in &new.nodes {
            let Some(o) = old.nodes.iter().find(|o| o.key == n.key) else {
                if n.live && !old.nodes.is_empty() {
                    events.push(Event::Live(n.key));
                }
                continue;
            };
            if let (Some(a), Some(b)) = (o.checked, n.checked) {
                if a != b {
                    events.push(match n.role {
                        Role::CheckBox => Event::Toggle(n.key, a, b),
                        _ => Event::Selected(n.key, a, b),
                    });
                }
            }
            if let (Some(a), Some(b)) = (&o.value, &n.value) {
                if a != b {
                    events.push(Event::Value(n.key, a.clone(), b.clone()));
                }
            }
            if n.live && o.name != n.name {
                events.push(Event::Live(n.key));
            }
        }
    }
    // Raised after the lock is released: clients may call straight back into the providers.
    if events.is_empty() || !unsafe { UiaClientsAreListening() }.as_bool() {
        return;
    }
    let we_have_focus = unsafe { GetFocus() } == hwnd;
    for e in events {
        unsafe {
            let _ = match e {
                Event::Focus(k) if we_have_focus => UiaRaiseAutomationEvent(&item(h, k), UIA_AutomationFocusChangedEventId),
                Event::Focus(_) => Ok(()),
                Event::Toggle(k, a, b) => UiaRaiseAutomationPropertyChangedEvent(
                    &item(h, k),
                    UIA_ToggleToggleStatePropertyId,
                    &var_i32(toggle_state(a).0),
                    &var_i32(toggle_state(b).0),
                ),
                Event::Selected(k, a, b) => {
                    UiaRaiseAutomationPropertyChangedEvent(&item(h, k), UIA_SelectionItemIsSelectedPropertyId, &var_bool(a), &var_bool(b))
                }
                Event::Value(k, a, b) => {
                    UiaRaiseAutomationPropertyChangedEvent(&item(h, k), UIA_ValueValuePropertyId, &var_str(&a), &var_str(&b))
                }
                Event::Live(k) => UiaRaiseAutomationEvent(&item(h, k), UIA_LiveRegionChangedEventId),
            };
        }
    }
}

/// Called from `ui::proc` on `WM_GETOBJECT` for `UiaRootObjectId`.
pub fn root_response(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let root: IRawElementProviderSimple = Root { hwnd: hwnd.0 as isize }.into();
    unsafe { UiaReturnRawElementProvider(hwnd, wp, lp, &root) }
}

/// Called from `ui::proc` on `WM_NCDESTROY`: drops the tree and releases UIA's references.
pub fn forget(hwnd: HWND) {
    let had = {
        let Ok(mut trees) = TREES.lock() else { return };
        let before = trees.len();
        trees.retain(|t| t.hwnd != hwnd.0 as isize);
        trees.len() != before
    };
    if had {
        unsafe {
            let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None::<&IRawElementProviderSimple>);
        }
    }
}

fn post(hwnd: isize, key: u32, a: Action) -> Result<()> {
    let raw = Box::into_raw(Box::new(a));
    unsafe {
        if let Err(e) = PostMessageW(Some(HWND(hwnd as *mut _)), WM_SNAPIT_A11Y, WPARAM(key as usize), LPARAM(raw as isize)) {
            drop(Box::from_raw(raw));
            return Err(e);
        }
    }
    Ok(())
}

// ---------- VARIANT helpers ----------

fn var(vt: windows::Win32::System::Variant::VARENUM, v: VARIANT_0_0_0) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 { vt, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: v }),
        },
    }
}

fn var_i32(v: i32) -> VARIANT {
    var(VT_I4, VARIANT_0_0_0 { lVal: v })
}

fn var_bool(v: bool) -> VARIANT {
    use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
    var(VT_BOOL, VARIANT_0_0_0 { boolVal: if v { VARIANT_TRUE } else { VARIANT_FALSE } })
}

/// A BSTR VARIANT. Returned to UI Automation, ownership passes to the caller; one Snap It
/// keeps (property-changed events) is freed by `VARIANT`'s own `Drop` (VariantClear).
fn var_str(s: &str) -> VARIANT {
    var(VT_BSTR, VARIANT_0_0_0 { bstrVal: std::mem::ManuallyDrop::new(BSTR::from(s)) })
}

fn toggle_state(on: bool) -> ToggleState {
    if on {
        ToggleState_On
    } else {
        ToggleState_Off
    }
}

fn control_type(r: Role) -> UIA_CONTROLTYPE_ID {
    match r {
        Role::Button => UIA_ButtonControlTypeId,
        Role::CheckBox => UIA_CheckBoxControlTypeId,
        Role::Edit | Role::Hotkey => UIA_EditControlTypeId,
        Role::RadioButton => UIA_RadioButtonControlTypeId,
        Role::TabItem => UIA_TabItemControlTypeId,
        Role::ListItem => UIA_ListItemControlTypeId,
        Role::Text => UIA_TextControlTypeId,
    }
}

fn supports(r: Role, pattern: UIA_PATTERN_ID) -> bool {
    match pattern {
        p if p == UIA_InvokePatternId => matches!(r, Role::Button | Role::ListItem),
        p if p == UIA_TogglePatternId => r == Role::CheckBox,
        p if p == UIA_ValuePatternId => matches!(r, Role::Edit | Role::Hotkey),
        p if p == UIA_SelectionItemPatternId => matches!(r, Role::RadioButton | Role::TabItem | Role::ListItem),
        _ => false,
    }
}

// ---------- providers ----------

/// The window itself. Name, control type and bounds come from the host (the HWND).
#[implement(IRawElementProviderSimple, IRawElementProviderFragment, IRawElementProviderFragmentRoot)]
struct Root {
    hwnd: isize,
}

/// One [`Node`].
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IInvokeProvider,
    IToggleProvider,
    IValueProvider,
    ISelectionItemProvider
)]
struct Item {
    hwnd: isize,
    key: u32,
}

fn item(hwnd: isize, key: u32) -> IRawElementProviderSimple {
    Item { hwnd, key }.into()
}

fn fragment(hwnd: isize, key: u32) -> IRawElementProviderFragment {
    Item { hwnd, key }.into()
}

fn root_fragment(hwnd: isize) -> IRawElementProviderFragmentRoot {
    Root { hwnd }.into()
}

/// "No such element / no value": S_OK with a null result, as UI Automation expects.
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

impl IRawElementProviderSimple_Impl for Root_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }
    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> Result<windows::core::IUnknown> {
        none()
    }
    fn GetPropertyValue(&self, _: UIA_PROPERTY_ID) -> Result<VARIANT> {
        Ok(VARIANT::default())
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as *mut _)) }
    }
}

impl IRawElementProviderFragment_Impl for Root_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let pick = |first: bool| {
            with_tree(self.hwnd, |t| if first { t.nodes.first() } else { t.nodes.last() }.map(|n| n.key)).flatten()
        };
        let key = match direction {
            d if d == NavigateDirection_FirstChild => pick(true),
            d if d == NavigateDirection_LastChild => pick(false),
            _ => None,
        };
        key.map(|k| fragment(self.hwnd, k)).map_or_else(none, Ok)
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(UiaRect::default())
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(root_fragment(self.hwnd))
    }
}

impl IRawElementProviderFragmentRoot_Impl for Root_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let mut p = POINT { x: x as i32, y: y as i32 };
        unsafe {
            let _ = ScreenToClient(HWND(self.hwnd as *mut _), &mut p);
        }
        let key = with_tree(self.hwnd, |t| {
            t.nodes.iter().rev().find(|n| !n.offscreen && n.rect.contains(p.x, p.y)).map(|n| n.key)
        })
        .flatten();
        key.map(|k| fragment(self.hwnd, k)).map_or_else(none, Ok)
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let key = with_tree(self.hwnd, |t| t.focus.filter(|k| t.nodes.iter().any(|n| n.key == *k))).flatten();
        key.map(|k| fragment(self.hwnd, k)).map_or_else(none, Ok)
    }
}

impl IRawElementProviderSimple_Impl for Item_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }
    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<windows::core::IUnknown> {
        let n = node(self.hwnd, self.key)?;
        if !supports(n.role, pattern) {
            return none();
        }
        let simple: IRawElementProviderSimple = Item { hwnd: self.hwnd, key: self.key }.into();
        simple.cast()
    }
    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let n = node(self.hwnd, self.key)?;
        let focused = with_tree(self.hwnd, |t| t.focus == Some(self.key)).unwrap_or(false);
        Ok(match property {
            p if p == UIA_ControlTypePropertyId => var_i32(control_type(n.role).0),
            p if p == UIA_NamePropertyId => var_str(&n.name),
            p if p == UIA_HelpTextPropertyId => n.help.as_deref().map(var_str).unwrap_or_default(),
            p if p == UIA_AutomationIdPropertyId => var_str(&format!("snapit-{}", self.key)),
            p if p == UIA_IsKeyboardFocusablePropertyId => var_bool(n.tab_stop && n.enabled),
            p if p == UIA_HasKeyboardFocusPropertyId => var_bool(focused),
            p if p == UIA_IsEnabledPropertyId => var_bool(n.enabled),
            p if p == UIA_IsOffscreenPropertyId => var_bool(n.offscreen),
            p if p == UIA_LiveSettingPropertyId && n.live => var_i32(Polite.0),
            p if p == UIA_FrameworkIdPropertyId => var_str("SnapIt"),
            _ => VARIANT::default(),
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}

impl IRawElementProviderFragment_Impl for Item_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        if direction == NavigateDirection_Parent {
            return root_fragment(self.hwnd).cast();
        }
        let key = with_tree(self.hwnd, |t| {
            let i = t.nodes.iter().position(|n| n.key == self.key)?;
            match direction {
                d if d == NavigateDirection_NextSibling => t.nodes.get(i + 1).map(|n| n.key),
                d if d == NavigateDirection_PreviousSibling => i.checked_sub(1).and_then(|j| t.nodes.get(j)).map(|n| n.key),
                _ => None,
            }
        })
        .flatten();
        key.map(|k| fragment(self.hwnd, k)).map_or_else(none, Ok)
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        unsafe {
            let sa = SafeArrayCreateVector(VT_I4, 0, 2);
            if sa.is_null() {
                return Err(Error::from(windows::Win32::Foundation::E_OUTOFMEMORY));
            }
            for (i, v) in [UiaAppendRuntimeId as i32, self.key as i32].iter().enumerate() {
                SafeArrayPutElement(sa, &(i as i32), v as *const i32 as *const _)?;
            }
            Ok(sa)
        }
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let n = node(self.hwnd, self.key)?;
        let mut p = POINT { x: n.rect.x, y: n.rect.y };
        unsafe {
            let _ = ClientToScreen(HWND(self.hwnd as *mut _), &mut p);
        }
        Ok(UiaRect { left: p.x as f64, top: p.y as f64, width: n.rect.w as f64, height: n.rect.h as f64 })
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        post(self.hwnd, self.key, Action::Focus)
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(root_fragment(self.hwnd))
    }
}

impl IInvokeProvider_Impl for Item_Impl {
    fn Invoke(&self) -> Result<()> {
        let n = node(self.hwnd, self.key)?;
        if !n.enabled {
            return Err(hr(UIA_E_ELEMENTNOTENABLED));
        }
        post(self.hwnd, self.key, Action::Invoke)
    }
}

impl IToggleProvider_Impl for Item_Impl {
    fn Toggle(&self) -> Result<()> {
        post(self.hwnd, self.key, Action::Toggle)
    }
    fn ToggleState(&self) -> Result<ToggleState> {
        Ok(toggle_state(node(self.hwnd, self.key)?.checked.unwrap_or(false)))
    }
}

impl IValueProvider_Impl for Item_Impl {
    fn SetValue(&self, val: &PCWSTR) -> Result<()> {
        let n = node(self.hwnd, self.key)?;
        if n.role != Role::Edit {
            return Err(Error::from(E_ACCESSDENIED));
        }
        let s = unsafe { val.to_string() }.unwrap_or_default();
        post(self.hwnd, self.key, Action::SetValue(s))
    }
    fn Value(&self) -> Result<BSTR> {
        Ok(BSTR::from(node(self.hwnd, self.key)?.value.unwrap_or_default()))
    }
    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok((node(self.hwnd, self.key)?.role != Role::Edit).into())
    }
}

impl ISelectionItemProvider_Impl for Item_Impl {
    fn Select(&self) -> Result<()> {
        post(self.hwnd, self.key, Action::Select)
    }
    fn AddToSelection(&self) -> Result<()> {
        post(self.hwnd, self.key, Action::Select)
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        Err(hr(UIA_E_INVALIDOPERATION))
    }
    fn IsSelected(&self) -> Result<BOOL> {
        Ok(node(self.hwnd, self.key)?.checked.unwrap_or(false).into())
    }
    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(key: u32, role: Role) -> Node {
        Node::new(key, Rect::new(0, 0, 10, 10), role, format!("n{key}"))
    }

    #[test]
    fn tab_order_skips_text_and_disabled_and_wraps() {
        let mut a = Access::default();
        a.push(n(1, Role::Text));
        a.push(n(2, Role::Button));
        a.push(n(3, Role::CheckBox).enabled(false));
        a.push(n(4, Role::Edit));
        a.push(n(5, Role::Button).no_tab());
        assert_eq!(a.next(false), Some(2), "first stop");
        a.focus = Some(2);
        assert_eq!(a.next(false), Some(4), "skips the disabled switch");
        a.focus = Some(4);
        assert_eq!(a.next(false), Some(2), "wraps");
        assert_eq!(a.next(true), Some(2));
        a.focus = Some(2);
        assert_eq!(a.next(true), Some(4), "Shift+Tab wraps backwards");
        a.focus = Some(3);
        assert_eq!(a.next(false), Some(4), "continues after a focused non-stop");
        assert_eq!(a.next(true), Some(2));
        a.nodes.clear();
        assert_eq!(a.next(false), None);
    }

    #[test]
    fn patterns_follow_roles() {
        assert!(supports(Role::Button, UIA_InvokePatternId));
        assert!(supports(Role::CheckBox, UIA_TogglePatternId));
        assert!(!supports(Role::CheckBox, UIA_InvokePatternId));
        assert!(supports(Role::Hotkey, UIA_ValuePatternId));
        assert!(supports(Role::TabItem, UIA_SelectionItemPatternId));
        assert!(!supports(Role::Text, UIA_ValuePatternId));
    }
}
