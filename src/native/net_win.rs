//! Per-process network throughput via ETW (Event Tracing for Windows).
//!
//! Listens to the Microsoft-Windows-Kernel-Network provider, the same source
//! Resource Monitor uses, and sums bytes sent/received per PID for TCP and UDP
//! over IPv4 and IPv6. Real-time ETW sessions require administrator rights.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferrisetw::parser::Parser;
use ferrisetw::provider::{EventFilter, Provider};
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, UserTrace};
use ferrisetw::EventRecord;

use super::NetStatus;

const SESSION: &str = "TaskForge-Network";
const KERNEL_NETWORK: &str = "7DD42A49-5329-4832-8DFD-43D979153A88";
/// KERNEL_NETWORK_KEYWORD_IPV4 | KERNEL_NETWORK_KEYWORD_IPV6
const KEYWORDS: u64 = 0x10 | 0x20;

type Counters = Arc<Mutex<HashMap<u32, (u64, u64)>>>;

pub struct NetMonitor {
    counters: Counters,
    trace: Option<UserTrace>,
}

impl NetMonitor {
    pub fn start() -> Result<NetMonitor, NetStatus> {
        if !crate::platform::is_admin() {
            return Err(NetStatus::NeedsAdmin);
        }
        let counters: Counters = Arc::new(Mutex::new(HashMap::new()));
        let c = counters.clone();
        let provider = Provider::by_guid(KERNEL_NETWORK)
            .any(KEYWORDS)
            .add_callback(
                move |record: &EventRecord, schema_locator: &SchemaLocator| {
                    // Event IDs: TCP send/recv v4 10/11, v6 26/27; UDP send/recv v4 42/43, v6 58/59.
                    let sent = matches!(record.event_id(), 10 | 26 | 42 | 58);
                    let recv = matches!(record.event_id(), 11 | 27 | 43 | 59);
                    if !sent && !recv {
                        return;
                    }
                    // Fast path: every TCP/UDP send/receive event starts with
                    // `PID: u32, size: u32`, so read them straight from the payload
                    // instead of a per-event schema lookup.
                    let (pid, size) = match fast_pid_size(record) {
                        Some(v) => v,
                        None => {
                            let Ok(schema) = schema_locator.event_schema(record) else {
                                return;
                            };
                            let parser = Parser::create(record, &schema);
                            (
                                parser
                                    .try_parse::<u32>("PID")
                                    .unwrap_or_else(|_| record.process_id()),
                                parser.try_parse::<u32>("size").unwrap_or(0),
                            )
                        }
                    };
                    if let Ok(mut m) = c.lock() {
                        let e = m.entry(pid).or_default();
                        if recv {
                            e.0 += size as u64;
                        } else {
                            e.1 += size as u64;
                        }
                    }
                },
            )
            // Only the send/receive events; the kernel drops the rest (connect, retransmit, …).
            .add_filter(EventFilter::ByEventIds(vec![
                10, 11, 26, 27, 42, 43, 58, 59,
            ]))
            .build();
        // A session left behind by a crashed instance would block ours.
        let _ = stop_trace_by_name(SESSION);
        let trace = UserTrace::new()
            .named(SESSION.to_string())
            .enable(provider)
            .start_and_process()
            .map_err(|e| NetStatus::Unavailable(format!("{e:?}")))?;
        Ok(NetMonitor {
            counters,
            trace: Some(trace),
        })
    }

    /// Bytes (received, sent) per PID since the previous call.
    pub fn take(&self) -> HashMap<u32, (u64, u64)> {
        self.counters
            .lock()
            .map(|mut m| std::mem::take(&mut *m))
            .unwrap_or_default()
    }
}

impl Drop for NetMonitor {
    fn drop(&mut self) {
        if let Some(t) = self.trace.take() {
            let _ = t.stop();
        }
    }
}

/// Stop the ETW session (called when the app closes).
pub fn shutdown() {
    let _ = stop_trace_by_name(SESSION);
}

/// Read `(PID, size)` from the start of a Kernel-Network send/receive payload.
fn fast_pid_size(record: &EventRecord) -> Option<(u32, u32)> {
    use windows::Win32::System::Diagnostics::Etw::EVENT_RECORD;
    // SAFETY: ferrisetw's `EventRecord` is `#[repr(transparent)]` over the Win32
    // EVENT_RECORD struct; our `windows` crate binds the same C layout. The payload
    // pointer/length are valid for the duration of the callback.
    let raw = unsafe { &*(record as *const EventRecord as *const EVENT_RECORD) };
    if raw.UserData.is_null() || raw.UserDataLength < 8 {
        return None;
    }
    let data = unsafe {
        std::slice::from_raw_parts(raw.UserData as *const u8, raw.UserDataLength as usize)
    };
    let pid = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let size = u32::from_le_bytes(data[4..8].try_into().ok()?);
    Some((pid, size))
}
