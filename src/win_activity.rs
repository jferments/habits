use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    hwnd: isize,
    pid: Option<u32>,
    image: Option<String>,
    idle: bool,
}

struct Current {
    state: State,
    started_at: DateTime<Utc>,
}

pub struct ForegroundMonitor {
    current: Option<Current>,
    idle_threshold_secs: u64,
}

impl ForegroundMonitor {
    pub fn new(idle_threshold_secs: u64) -> Self {
        Self { current: None, idle_threshold_secs }
    }

    pub fn sample(&mut self, conn: &Connection) -> Result<()> {
        let now = Utc::now();
        let idle = user_idle_seconds().map(|s| s >= self.idle_threshold_secs).unwrap_or(false);
        // SAFETY: Querying current foreground HWND and its owning process is read-only.
        let hwnd = unsafe { GetForegroundWindow() };
        let mut pid = 0_u32;
        if !hwnd.is_null() {
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid); }
        }
        let raw_handle = hwnd as isize;
        let process_id = (pid != 0).then_some(pid);
        let image = if process_id.is_some() && self.current.as_ref().is_some_and(
            |c| c.state.hwnd == raw_handle && c.state.pid == process_id
        ) {
            self.current.as_ref().unwrap().state.image.clone()
        } else {
            process_id.and_then(process_image)
        };
        let state = State { hwnd: raw_handle, pid: process_id, image, idle };
        let changed = self.current.as_ref().is_none_or(|c| c.state != state);
        // Periodic flush bounds data lost if the process is killed without shutdown.
        let long = self.current.as_ref().is_some_and(|c| (now - c.started_at).num_seconds() >= 60);
        if changed || long {
            self.flush_at(conn, now)?;
            self.current = Some(Current { state, started_at: now });
        }
        Ok(())
    }

    pub fn flush(&mut self, conn: &Connection) -> Result<()> {
        self.flush_at(conn, Utc::now())
    }

    fn flush_at(&mut self, conn: &Connection, until: DateTime<Utc>) -> Result<()> {
        if let Some(current) = self.current.take() {
            if until > current.started_at && current.state.pid.is_some() {
                conn.execute(r#"INSERT INTO activity_intervals
                    (started_utc,ended_utc,pid,image,idle) VALUES(?1,?2,?3,?4,?5)"#,
                    params![current.started_at.to_rfc3339_opts(SecondsFormat::Millis, true), until.to_rfc3339_opts(SecondsFormat::Millis, true),
                        current.state.pid.map(i64::from), current.state.image,
                        i64::from(current.state.idle)])?;
            }
        }
        Ok(())
    }
}

fn user_idle_seconds() -> Option<u64> {
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: valid LASTINPUTINFO pointer, size field set as required by Win32.
    if unsafe { GetLastInputInfo(&mut info) } == 0 { return None; }
    // LASTINPUTINFO.dwTime is DWORD uptime modulo 2^32; subtraction wraps.
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    Some((now as u32).wrapping_sub(info.dwTime) as u64 / 1000)
}

fn process_image(pid: u32) -> Option<String> {
    // SAFETY: OpenProcess / QueryFullProcessImageNameW only request query permissions.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() { return None; }
    let mut buf = vec![0_u16; 32768];
    let mut chars = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut chars) };
    unsafe { CloseHandle(handle); }
    if ok == 0 { return None; }
    let path = String::from_utf16_lossy(&buf[..chars as usize]);
    // Avoid recording window titles or user documents; only the binary path is stored.
    if Path::new(&path).file_name().is_none() { return None; }
    Some(path)
}
