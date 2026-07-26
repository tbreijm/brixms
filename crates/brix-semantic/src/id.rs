//! `digest_id!` — the shared shape of a content-addressed artifact identity: a
//! distinct newtype over a [`brix_canon::Digest`], hashed under
//! [`brix_canon::Domain::Value`], with the same `from_canon` / `digest` /
//! `to_hex` / `Canonical` surface. Each artifact adds its own semantic
//! constructor (`of`, `named`, …) on top.
//!
//! ([`crate::ContextId`] predates this macro and stays hand-written — it
//! carries the special `root()` migration anchor.)

/// Define a `Digest`-newtype identity with the common surface. Distinct types
/// over the same representation so one id cannot be passed where another is
/// wanted (spec Part III §3, mirroring `brix-canon`'s own `NodeId`/`EdgeId`).
macro_rules! digest_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub ::brix_canon::Digest);

        impl $name {
            /// Hash a canon-encoded payload under the value domain.
            pub fn from_canon(payload: &[u8]) -> Self {
                $name(::brix_canon::Digest::of(::brix_canon::Domain::Value, payload))
            }

            /// The content-addressed id of any canonically-encodable value.
            pub fn of(value: &impl ::brix_canon::Canonical) -> Self {
                let mut w = ::brix_canon::CanonWriter::new();
                value.canon_write(&mut w);
                $name::from_canon(&w.finish())
            }

            /// The underlying digest.
            pub fn digest(&self) -> ::brix_canon::Digest {
                self.0
            }

            /// Lowercase-hex rendering (diagnostics / `brix why`).
            pub fn to_hex(&self) -> String {
                self.0.to_hex()
            }
        }

        impl ::brix_canon::Canonical for $name {
            fn canon_write(&self, w: &mut ::brix_canon::CanonWriter) {
                w.write_bytes(self.0.as_bytes());
            }
        }
    };
}

pub(crate) use digest_id;
