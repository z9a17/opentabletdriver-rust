//! Original managed source parsers. Native profiles retain the inline decoder;
//! managed report consumers receive the original concrete object in the graph.
use super::{Graph, GraphReport, bridge, last_error};
use super::graph::Callback;
use std::ffi::c_void;
use std::io;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;
use otd_core::decoders::{DecodeError, DecodedInput, DecodedPen, PenDecoder, TabletDecoder, pen_from_values};
use otd_core::reports::{DeviceId, EndpointId, ManagedReportToken, ReportEnvelope, ReportMetadata, SessionId};
use otd_core::spec::TabletSpec;

#[repr(C)]
struct ParsedSourceReport { report: GraphReport, token: ManagedReportToken }
type CreateForGraph = unsafe extern "C" fn(*mut c_void, *const u8, u32) -> *mut c_void;
type Parse = unsafe extern "C" fn(*mut c_void, *const u8, u32, *mut ParsedSourceReport) -> i32;
type Dispatch = unsafe extern "C" fn(*mut c_void, u64, u64, *const GraphReport, Callback, *mut c_void, i32) -> i32;
pub(super) struct Api { create: CreateForGraph, parse: Parse, dispatch: Dispatch }
impl Api {
    pub(super) fn load(entry: &impl Fn(&str) -> Result<*mut c_void, String>) -> Result<Self, String> {
        Ok(unsafe { Self { create: std::mem::transmute::<*mut c_void, CreateForGraph>(entry("CreateHostedGraphParser")?), parse: std::mem::transmute::<*mut c_void, Parse>(entry("ParseSourceReport")?),
            dispatch: std::mem::transmute::<*mut c_void, Dispatch>(entry("DispatchParsedGraph")?) } })
    }
}
fn api() -> Result<&'static Api, String> { bridge()?.parser.as_ref().ok_or_else(|| "The installed .NET bridge lacks original source parser graph support; replace data/compat with this release's files.".into()) }

pub struct ManagedReportParser {
    context: *mut c_void,
    spec: TabletSpec,
    reset_failure: Option<String>,
    _owner_thread: PhantomData<Rc<()>>,
}
impl ManagedReportParser {
    pub fn for_graph(name: &str, spec: TabletSpec, graph: &Graph) -> Result<Self, String> {
        Self::for_source(name,spec,Some(graph),false)
    }
    fn for_source(name:&str,spec:TabletSpec,graph:Option<&Graph>,auxiliary:bool)->Result<Self,String>{
        if name.is_empty() || name.len() > 4096 { return Err("Managed parser name length must be 1..4096".into()); }
        let mut source=super::source_session_json();
        if let Some(source)=source.as_mut().and_then(serde_json::Value::as_object_mut){source.insert("auxiliary".into(),serde_json::json!(auxiliary));}
        let json=serde_json::json!({"parser":name,"source_session":source}).to_string();
        let context = unsafe { (api()?.create)(graph.map_or(std::ptr::null_mut(),Graph::context_handle), json.as_ptr(), json.len() as u32) };
        if context.is_null() { return Err(last_error()); }
        Ok(Self { context, spec, reset_failure: None, _owner_thread: PhantomData })
    }
    pub fn new(name: &str, spec: TabletSpec) -> Result<Self, String> {
        if name.is_empty() || name.len() > 4096 { return Err("Managed parser name length must be 1..4096".into()); }
        let json=serde_json::json!({"parser":name,"source_session":super::source_session_json()}).to_string();
        let context=unsafe{(api()?.create)(std::ptr::null_mut(),json.as_ptr(),json.len() as u32)};
        if context.is_null(){return Err(last_error());}
        Ok(Self { context, spec, reset_failure: None, _owner_thread: PhantomData })
    }
}
impl PenDecoder for ManagedReportParser {
    fn decode<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedPen<'a>>, DecodeError> {
        Ok(self.decode_input(raw)?.and_then(|input| input.pen().map(|pen| DecodedPen { pen, raw, buttons: match input { DecodedInput::Report { report, .. } => report.values.pen_buttons, DecodedInput::Pen(value) => value.buttons } })))
    }
    fn decode_input<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedInput<'a>>, DecodeError> {
        if let Some(error) = &self.reset_failure { return Err(DecodeError::Managed(error.clone())); }
        if raw.is_empty() || raw.len() > 65535 { return Err(DecodeError::Managed("Managed source packet length must be 1..65535".into())); }
        let mut projected = std::mem::MaybeUninit::<ParsedSourceReport>::uninit();
        let code = unsafe { (api().map_err(DecodeError::Managed)?.parse)(self.context, raw.as_ptr(), raw.len() as u32, projected.as_mut_ptr()) };
        if code < 0 { return Err(DecodeError::Managed(last_error())); }
        if code == 0 { return Ok(None); }
        if code != 1 { return Err(DecodeError::Managed("Invalid managed source emission status".into())); }
        let projected = unsafe { projected.assume_init() };
        let (kind, mut values) = projected.report.decode().map_err(|error| DecodeError::Managed(error.to_string()))?;
        if projected.token.parser == 0 || projected.token.sequence == 0 { return Err(DecodeError::Managed("Invalid managed source identity".into())); }
        values.managed_report = Some(projected.token);
        let pen = pen_from_values(kind, &values, raw, self.spec);
        Ok(Some(DecodedInput::Report { kind, report: ReportEnvelope {
            metadata: ReportMetadata { device: DeviceId(0), session: SessionId(0), endpoint: EndpointId(0), received_at: Duration::ZERO, sequence: 0 },
            // Transport bytes remain borrowed safely. The graph takes the exact
            // original object, including its parser-owned Raw, by checked token.
            raw, values,
        }, pen }))
    }
    fn reset(&mut self) { self.reset_failure = super::registry::reset_parser(self.context).err(); }
}
impl Drop for ManagedReportParser { fn drop(&mut self) { super::registry::destroy_parser(self.context); } }

pub enum RuntimeDecoder { Native(TabletDecoder), Managed(ManagedReportParser) }
impl RuntimeDecoder {
    /// Preserve native decoding whenever no unchanged report consumer needs
    /// an original object; fall back only for an actually registered parser.
    pub fn for_parser(name: &str, spec: TabletSpec) -> Result<Self, String> { Self::for_graph(name, spec, false) }
    pub fn for_graph(name: &str, spec: TabletSpec, prefer_original: bool) -> Result<Self, String> {
        if !prefer_original && let Some(native) = TabletDecoder::for_parser(name, spec) { return Ok(Self::Native(native)); }
        ManagedReportParser::new(name, spec).map(Self::Managed)
    }
    pub fn for_pipeline(name: &str, spec: TabletSpec, graph: Option<&Graph>) -> Result<Self, String> {
        Self::for_pipeline_source(name,spec,graph,false)
    }
    pub fn for_pipeline_source(name:&str,spec:TabletSpec,graph:Option<&Graph>,auxiliary:bool)->Result<Self,String>{
        match graph {
            Some(graph) => ManagedReportParser::for_source(name, spec, Some(graph),auxiliary).map(Self::Managed),
            None => match TabletDecoder::for_parser(name,spec){Some(native)=>Ok(Self::Native(native)),None=>ManagedReportParser::for_source(name,spec,None,auxiliary).map(Self::Managed)},
        }
    }
    pub fn is_managed(&self) -> bool { matches!(self, Self::Managed(_)) }
}
impl PenDecoder for RuntimeDecoder {
    fn decode<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedPen<'a>>, DecodeError> { match self { Self::Native(value) => value.decode(raw), Self::Managed(value) => value.decode(raw) } }
    fn decode_input<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedInput<'a>>, DecodeError> { match self { Self::Native(value) => value.decode_input(raw), Self::Managed(value) => value.decode_input(raw) } }
    fn reset(&mut self) { match self { Self::Native(value) => value.reset(), Self::Managed(value) => value.reset() } }
}

impl Graph {
    /// The source report is consumed once by its owner thread; stale tokens
    /// fail before any native continuation or plugin executes.
    pub unsafe fn dispatch_parsed(&self, token: ManagedReportToken, frame: &GraphReport, callback: Callback, scope: *mut c_void) -> io::Result<()> {
        let code = unsafe { (api().map_err(io::Error::other)?.dispatch)(self.context_handle(), token.parser, token.sequence, frame, callback, scope, i32::from(self.runs_builtins_in_host())) };
        if code == 0 { Ok(()) } else { Err(io::Error::other(last_error())) }
    }
}

/// Metadata-only discovery, without constructing a parser or initializing CLR.
pub fn installed_report_parser(name: &str) -> bool {
    super::registry_snapshot().is_some_and(|registry| registry.plugins.iter().filter(|entry| entry.metadata.category == "parser" && entry.metadata.supported && entry.config.type_name == name).count() == 1)
}
