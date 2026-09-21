//! Tracing subscriber setup, with optional OTLP export (FRD-018 M6).
//!
//! Before this, WaaV called `tracing_subscriber::fmt::init()` and nothing else — so the two
//! OTel variables the Helm chart sets (`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and
//! `OTEL_RESOURCE_ATTRIBUTES`) were read by nobody. The deployment looked instrumented and
//! exported nothing, which is the worst of both: an operator sees the configuration and
//! concludes the absence of traces means the absence of traffic.
//!
//! Export is OPT-IN on the endpoint variable. Unset means stdout logging exactly as before, so
//! a standalone WaaV is unchanged and no deployment starts shipping spans by surprise.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Held by `main` for the process lifetime so shutdown can flush.
///
/// Dropping this without calling [`shutdown`] loses whatever is still batched — which in
/// practice is every span from the request that triggered the shutdown, precisely the ones
/// someone debugging a crash wants.
pub struct TracingGuard {
    provider: Option<SdkTracerProvider>,
}

impl TracingGuard {
    /// Flush pending spans. Call before exiting.
    pub fn shutdown(&mut self) {
        if let Some(provider) = self.provider.take()
            && let Err(e) = provider.shutdown()
        {
            eprintln!("failed to flush traces on shutdown: {e}");
        }
    }
}

/// Parse `OTEL_RESOURCE_ATTRIBUTES` (`k=v,k=v`), the format the chart writes.
///
/// Malformed pairs are skipped rather than fatal: losing one resource label is a worse reason
/// to refuse to start than to carry on without it.
fn resource_attributes(raw: &str) -> Vec<KeyValue> {
    raw.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            if k.is_empty() || v.is_empty() {
                return None;
            }
            Some(KeyValue::new(k.to_string(), v.to_string()))
        })
        .collect()
}

/// Install the subscriber, exporting to OTLP when an endpoint is configured.
pub fn init() -> TracingGuard {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .ok()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty());

    let Some(endpoint) = endpoint else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .init();
        tracing::info!("OTLP export not configured; tracing to stdout only");
        return TracingGuard { provider: None };
    };

    let mut attrs = std::env::var("OTEL_RESOURCE_ATTRIBUTES")
        .map(|raw| resource_attributes(&raw))
        .unwrap_or_default();
    // Only supply a default service.name when the environment did not: overriding an
    // operator's chosen name would split one service across two names in the trace UI.
    if !attrs.iter().any(|kv| kv.key.as_str() == "service.name") {
        attrs.push(KeyValue::new("service.name", "waav"));
    }

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint.clone())
        .with_timeout(Duration::from_secs(5))
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            // NOT fatal. A collector that is down must not stop the gateway serving audio;
            // losing telemetry is bad, refusing calls because telemetry is unavailable is worse.
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer())
                .init();
            tracing::error!(error = %e, endpoint = %endpoint, "OTLP exporter build failed; tracing to stdout only");
            return TracingGuard { provider: None };
        }
    };

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::builder().with_attributes(attrs).build())
        .build();

    let tracer = provider.tracer("waav-gateway");
    opentelemetry::global::set_tracer_provider(provider.clone());

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();

    tracing::info!(endpoint = %endpoint, "OTLP trace export enabled");
    TracingGuard {
        provider: Some(provider),
    }
}

#[cfg(test)]
mod tests {
    use super::resource_attributes;

    #[test]
    fn parses_the_comma_separated_form_the_chart_writes() {
        let attrs = resource_attributes(
            "service.namespace=bud,deployment.environment.name=dev,service.version=frd018-7",
        );
        let names: Vec<_> = attrs.iter().map(|kv| kv.key.as_str().to_string()).collect();
        assert_eq!(
            names,
            vec![
                "service.namespace",
                "deployment.environment.name",
                "service.version"
            ]
        );
    }

    #[test]
    fn a_malformed_pair_is_skipped_rather_than_taking_the_rest_with_it() {
        // Losing one label is a far worse reason to refuse to start than carrying on without it.
        let attrs = resource_attributes("good=1,broken,alsogood=2,=novalue,noKey=");
        let names: Vec<_> = attrs.iter().map(|kv| kv.key.as_str().to_string()).collect();
        assert_eq!(names, vec!["good", "alsogood"]);
    }

    #[test]
    fn whitespace_around_a_pair_is_tolerated() {
        let attrs = resource_attributes(" a = 1 , b = 2 ");
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].key.as_str(), "a");
        assert_eq!(attrs[0].value.as_str(), "1");
    }

    #[test]
    fn an_empty_string_yields_no_attributes_rather_than_one_blank() {
        assert!(resource_attributes("").is_empty());
    }

    #[test]
    fn a_value_containing_an_equals_sign_survives() {
        // service.version can carry a tag with '='; splitting on the FIRST '=' only is why.
        let attrs = resource_attributes("service.version=sha=abc123");
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].value.as_str(), "sha=abc123");
    }
}
