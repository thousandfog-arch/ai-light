#[cfg(target_os = "windows")]
mod platform {
    use std::ffi::c_void;
    use windows::core::Result as WindowsResult;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElementArray,
        IUIAutomationSelectionItemPattern, TreeScope_Descendants,
        UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, SetForegroundWindow, ShowWindowAsync, SW_RESTORE,
    };

    enum ComApartment {
        Owned,
        Borrowed,
    }

    impl ComApartment {
        fn initialize() -> Option<Self> {
            let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if result.is_ok() {
                Some(Self::Owned)
            } else if result.0 as u32 == 0x8001_0106 {
                // COM was already initialized on this thread with another model.
                // UI Automation can still use that existing apartment.
                Some(Self::Borrowed)
            } else {
                None
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if matches!(self, Self::Owned) {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn hwnd(value: i64) -> Option<HWND> {
        if value <= 0 {
            return None;
        }
        let handle = HWND(value as isize as *mut c_void);
        unsafe { IsWindow(Some(handle)) }.as_bool().then_some(handle)
    }

    unsafe fn terminal_tabs(
        automation: &IUIAutomation,
        window: HWND,
    ) -> WindowsResult<IUIAutomationElementArray> {
        let root = automation.ElementFromHandle(window)?;
        let condition = automation.CreateTrueCondition()?;
        root.FindAll(TreeScope_Descendants, &condition)
    }

    pub fn selected_terminal_tab_index(host_window: i64) -> Option<usize> {
        let window = hwnd(host_window)?;
        let _apartment = ComApartment::initialize()?;
        let automation: IUIAutomation = unsafe {
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?
        };
        let elements = unsafe { terminal_tabs(&automation, window).ok()? };
        let length = unsafe { elements.Length().ok()? };
        let mut terminal_index = 0usize;
        for index in 0..length {
            let element = unsafe { elements.GetElement(index).ok()? };
            if unsafe { element.CurrentControlType().ok()? } != UIA_TabItemControlTypeId {
                continue;
            }
            let pattern: IUIAutomationSelectionItemPattern = unsafe {
                element.GetCurrentPatternAs(UIA_SelectionItemPatternId).ok()?
            };
            if unsafe { pattern.CurrentIsSelected().ok()? }.as_bool() {
                return Some(terminal_index);
            }
            terminal_index += 1;
        }
        None
    }

    pub fn select_terminal_tab(host_window: i64, tab_index: usize) -> bool {
        let Some(window) = hwnd(host_window) else {
            return false;
        };
        let Some(_apartment) = ComApartment::initialize() else {
            return false;
        };
        let Ok(automation): WindowsResult<IUIAutomation> = (unsafe {
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        }) else {
            return false;
        };
        let Ok(elements) = (unsafe { terminal_tabs(&automation, window) }) else {
            return false;
        };
        let Ok(length) = (unsafe { elements.Length() }) else {
            return false;
        };
        let mut terminal_index = 0usize;
        for index in 0..length {
            let Ok(element) = (unsafe { elements.GetElement(index) }) else {
                continue;
            };
            if unsafe { element.CurrentControlType() }.ok() != Some(UIA_TabItemControlTypeId) {
                continue;
            }
            if terminal_index == tab_index {
                let Ok(pattern): WindowsResult<IUIAutomationSelectionItemPattern> = (unsafe {
                    element.GetCurrentPatternAs(UIA_SelectionItemPatternId)
                }) else {
                    return false;
                };
                return unsafe { pattern.Select() }.is_ok();
            }
            terminal_index += 1;
        }
        false
    }

    pub fn focus_window(host_window: i64) -> bool {
        let Some(window) = hwnd(host_window) else {
            return false;
        };
        unsafe {
            let _ = ShowWindowAsync(window, SW_RESTORE);
            SetForegroundWindow(window).as_bool()
        }
    }
}

#[cfg(target_os = "windows")]
pub use platform::{focus_window, select_terminal_tab, selected_terminal_tab_index};

#[cfg(not(target_os = "windows"))]
pub fn selected_terminal_tab_index(_host_window: i64) -> Option<usize> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn select_terminal_tab(_host_window: i64, _tab_index: usize) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn focus_window(_host_window: i64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_window_is_rejected() {
        assert_eq!(selected_terminal_tab_index(0), None);
        assert!(!select_terminal_tab(0, 0));
        assert!(!focus_window(0));
    }
}
