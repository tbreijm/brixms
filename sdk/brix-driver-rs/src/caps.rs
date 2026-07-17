//! Capability handles a Driver may hold (Part VII §4: `Net<HostPattern>`,
//! `Fs<Root, Mode>`, `Clock`, `Random<Alg>`, `GraphReader<Scope>`,
//! `GraphWriter<Scope>`, `Console`).
//!
//! These trait definitions mirror the `capabilities` interface in
//! `sdk/driver-wit/delta-abi.wit`. They are Rust traits (not the generated
//! `wit-bindgen` resource types) on purpose: a Driver's `on_request` body is
//! written against them today, and when the wasmtime host + generated
//! bindings land, the host-side `capabilities` resource implements these same
//! traits — the Driver code does not change. Only [`Net`] and [`Console`] are
//! modeled here; that is exactly the surface the HTTP-notify template needs,
//! and the rest are named in the WIT world for when their Drivers arrive.

/// One outbound request over a [`Net`] capability. Mirrors WIT
/// `capabilities.http-request`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HttpRequest {
    /// HTTP method (`"POST"`, `"GET"`, ...).
    pub method: String,
    /// Absolute target URL. The host validates it against the capability's
    /// `HostPattern` scope; the guest cannot widen that scope.
    pub url: String,
    /// Request headers, in the order the Driver set them.
    pub headers: Vec<(String, String)>,
    /// Raw request body.
    pub body: Vec<u8>,
}

/// A response from a [`Net`] request. Mirrors WIT `capabilities.http-response`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Raw response body.
    pub body: Vec<u8>,
}

/// Why a [`Net`] request did not produce a response. Mirrors WIT
/// `capabilities.net-error`. Every variant is a value a Driver turns into an
/// honest [`crate::Outcome`] — never a panic (Part VII: a lossy boundary must
/// name and record its loss).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NetError {
    /// The host refused the target as outside the capability's scope.
    Forbidden(String),
    /// Transport failure; the string is a diagnostic.
    Transport(String),
    /// The host cancelled the in-flight request (lease expiry / shutdown).
    Cancelled,
}

/// `Net<HostPattern>` (Part VII §4). Host-issued and affine; the guest may
/// only reach hosts the issued handle's pattern allows.
pub trait Net {
    /// Perform one outbound request.
    fn request(&self, req: &HttpRequest) -> Result<HttpResponse, NetError>;
}

/// `Console` (Part VII §4) — dev/scenario logging only, no production
/// semantics.
pub trait Console {
    /// Emit one log line.
    fn log(&self, message: &str);
}
