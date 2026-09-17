use serde::Serialize;
use serde_json::json;
use std::time::Instant;
use uuid::Uuid;

#[derive(Clone)]
pub struct TraceContext {
    pub trace_id: String,
}

impl TraceContext {
    pub fn new(operation: &str) -> Self {
        let trace = Self { trace_id: Uuid::new_v4().to_string() };
        emit(json!({"event":"trace.start","trace_id":trace.trace_id,"operation":operation}));
        trace
    }

    pub fn span(&self, name: &str) -> TraceSpan {
        let span = TraceSpan { trace_id: self.trace_id.clone(), span_id: Uuid::new_v4().to_string(), name: name.to_owned(), started: Instant::now(), outcome: "ok".into() };
        emit(json!({"event":"span.start","trace_id":span.trace_id,"span_id":span.span_id,"span":span.name}));
        span
    }

    pub fn error(&self, message: &str) {
        emit(json!({"event":"trace.error","trace_id":self.trace_id,"message":message}));
    }
}

pub struct TraceSpan {
    trace_id: String,
    span_id: String,
    name: String,
    started: Instant,
    outcome: String,
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        emit(json!({"event":"span.end","trace_id":self.trace_id,"span_id":self.span_id,"span":self.name,"duration_ms":self.started.elapsed().as_secs_f64()*1000.0,"outcome":self.outcome}));
    }
}

fn emit<T: Serialize>(value: T) {
    eprintln!("{}", serde_json::to_string(&value).unwrap_or_else(|_| "{\"event\":\"trace.serialization_error\"}".into()));
}
