use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};

#[derive(Clone)]
pub(crate) struct Metrics {
    registry: Registry,
    pub(crate) requests: IntCounterVec,
    pub(crate) duration: HistogramVec,
    pub(crate) active: IntGauge,
    pub(crate) queued: IntGauge,
    pub(crate) rewrite_input_bytes: IntCounterVec,
    pub(crate) rewrite_output_bytes: IntCounterVec,
}

impl Metrics {
    pub(crate) fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new("kse_rewrite_requests_total", "Requests handled by outcome"),
            &["outcome", "extension"],
        )?;
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "kse_rewrite_request_duration_seconds",
                "End-to-end request duration",
            ),
            &["outcome"],
        )?;
        let active = IntGauge::new("kse_rewrite_active", "Active response rewrites")?;
        let queued = IntGauge::new("kse_rewrite_queued", "Queued response rewrites")?;
        let rewrite_input_bytes = IntCounterVec::new(
            Opts::new("kse_rewrite_input_bytes_total", "Decoded bytes inspected"),
            &["extension"],
        )?;
        let rewrite_output_bytes = IntCounterVec::new(
            Opts::new("kse_rewrite_output_bytes_total", "Rewritten bytes emitted"),
            &["extension"],
        )?;

        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(duration.clone()))?;
        registry.register(Box::new(active.clone()))?;
        registry.register(Box::new(queued.clone()))?;
        registry.register(Box::new(rewrite_input_bytes.clone()))?;
        registry.register(Box::new(rewrite_output_bytes.clone()))?;
        Ok(Self {
            registry,
            requests,
            duration,
            active,
            queued,
            rewrite_input_bytes,
            rewrite_output_bytes,
        })
    }

    pub(crate) fn encode(&self) -> Result<(String, Vec<u8>), prometheus::Error> {
        let encoder = TextEncoder::new();
        let mut body = Vec::new();
        encoder.encode(&self.registry.gather(), &mut body)?;
        Ok((encoder.format_type().to_owned(), body))
    }
}
