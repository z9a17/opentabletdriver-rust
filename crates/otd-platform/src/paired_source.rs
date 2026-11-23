//! Two independently owned endpoint parsers share one physical output owner.
//! The platform waits on both endpoints without moving either native handle.
use otd_core::session::{Read, ReportSource};
use std::{
    io,
    time::{Duration, Instant},
};
/// Which endpoint(s) became ready during a combined wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitWake {
    pub primary: bool,
    pub auxiliary: bool,
}
impl WaitWake {
    pub const BOTH: Self = Self {
        primary: true,
        auxiliary: true,
    };
    pub const NONE: Self = Self {
        primary: false,
        auxiliary: false,
    };
}

enum ResultKind {
    Report(usize, Instant, bool),
    Idle,
    Ended,
}
pub trait Retire {
    fn retire(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub struct PairedSource<S, W> {
    primary: S,
    auxiliary: Option<S>,
    wait: W,
    delivery: Box<[u8]>,
    aux_first: bool,
}
impl<S, W> PairedSource<S, W> {
    pub fn new(primary: S, auxiliary: Option<S>, wait: W) -> Self {
        // Only allocate the paired copy buffer when an auxiliary endpoint exists;
        // single-endpoint sessions borrow directly from `primary`.
        let delivery = if auxiliary.is_some() {
            vec![0; 65535].into_boxed_slice()
        } else {
            Box::new([])
        };
        Self {
            primary,
            auxiliary,
            wait,
            delivery,
            aux_first: false,
        }
    }
}
impl<S, W> PairedSource<S, W> {
    pub fn close(&mut self, mut close: impl FnMut(&mut S) -> io::Result<()>) -> io::Result<()> {
        let primary = close(&mut self.primary);
        let auxiliary = self.auxiliary.as_mut().map(&mut close).transpose();
        primary.and(auxiliary.map(|_| ()))
    }
}
fn copy(source: &mut impl ReportSource, delivery: &mut [u8]) -> io::Result<ResultKind> {
    match source.next(Duration::ZERO)? {
        Read::Report {
            bytes,
            ready,
            queued,
        }
        | Read::Auxiliary {
            bytes,
            ready,
            queued,
        } => {
            if bytes.len() > delivery.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Endpoint exceeds paired report bound",
                ));
            }
            delivery[..bytes.len()].copy_from_slice(bytes);
            Ok(ResultKind::Report(bytes.len(), ready, queued))
        }
        Read::Ended | Read::AuxiliaryEnded => Ok(ResultKind::Ended),
        Read::Idle => Ok(ResultKind::Idle),
    }
}
impl<S: ReportSource + Retire, W: FnMut(&mut S, Option<&mut S>, Duration) -> io::Result<WaitWake>>
    ReportSource for PairedSource<S, W>
{
    fn label(&self) -> &str {
        self.primary.label()
    }
    fn now(&self) -> Instant {
        self.primary.now()
    }
    fn shared_output(&self) -> bool {
        self.primary.shared_output()
    }
    fn native_output_enabled(&self) -> bool {
        self.primary.native_output_enabled()
    }
    fn output_started(&self) {
        self.primary.output_started()
    }
    fn output_acknowledged(&self, enabled: bool) {
        self.primary.output_acknowledged(enabled)
    }
    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        // Zero-copy fast path: without an auxiliary endpoint, delegate directly
        // to `primary` so it owns its own wait/read with no extra poll or copy.
        if self.auxiliary.is_none() {
            return self.primary.next(timeout);
        }
        let mut deadline: Option<Instant> = None;
        let mut check_primary = true;
        let mut check_auxiliary = true;
        let mut waited = false;
        loop {
            // Rotate priority when both streams are continuously readable.
            for auxiliary in [self.aux_first, !self.aux_first] {
                let result = if auxiliary {
                    if !check_auxiliary {
                        continue;
                    }
                    match &mut self.auxiliary {
                        Some(source) => copy(source, &mut self.delivery),
                        None => continue,
                    }
                } else {
                    if !check_primary {
                        continue;
                    }
                    copy(&mut self.primary, &mut self.delivery)
                };
                match result {
                    Ok(ResultKind::Report(length, ready, queued)) => {
                        self.aux_first = !auxiliary;
                        return Ok(if auxiliary {
                            Read::Auxiliary {
                                bytes: &self.delivery[..length],
                                ready,
                                queued: !waited && queued,
                            }
                        } else {
                            Read::Report {
                                bytes: &self.delivery[..length],
                                ready,
                                queued: !waited && queued,
                            }
                        });
                    }
                    Err(error) if auxiliary => {
                        eprintln!("Auxiliary endpoint retired: {error}");
                        if let Some(source) = &mut self.auxiliary {
                            source.retire()?;
                        }
                        self.auxiliary = None;
                        return Ok(Read::AuxiliaryEnded);
                    }
                    Ok(ResultKind::Ended) if auxiliary => {
                        if let Some(source) = &mut self.auxiliary {
                            source.retire()?;
                        }
                        self.auxiliary = None;
                        return Ok(Read::AuxiliaryEnded);
                    }
                    Ok(ResultKind::Ended) => return Ok(Read::Ended),
                    Err(error) => return Err(error),
                    Ok(ResultKind::Idle) => {}
                }
            }
            if timeout.is_zero() {
                return Ok(Read::Idle);
            }
            let now = Instant::now();
            let deadline = *deadline.get_or_insert_with(|| now + timeout);
            let remaining = deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                return Ok(Read::Idle);
            }
            let wake = (self.wait)(&mut self.primary, self.auxiliary.as_mut(), remaining)?;
            waited = true;
            // Output authority changes wake even when neither endpoint emits
            // a report. Core performs its release/reset before acknowledging.
            if !self.primary.native_output_enabled() {
                return Ok(Read::Idle);
            }
            if !wake.primary && !wake.auxiliary {
                check_primary = true;
                check_auxiliary = true;
                continue;
            }
            check_primary = wake.primary;
            check_auxiliary = wake.auxiliary;
        }
    }
}
