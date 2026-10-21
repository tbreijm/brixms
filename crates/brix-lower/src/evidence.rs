//! Evidence profile `brix.evidence@1` (ADR-0048).
//!
//! This checker admits nominal types, the initial integer refinement fragment,
//! function contracts, and explicit `validate` boundaries. A satisfied predicate
//! is a runtime or bounded-fragment check. It does not mint a proof-kernel
//! certificate, and a missing or rejected proof does not become `Refuted`.
//!
//! Programs that do not use these forms never enter this checker. Finite-decision
//! program identity is unchanged because that profile rejects these items.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{Digest, Domain};
use brix_kernel::{acceptance, Budget, ExplicitTerm, Prop, Verdict};
use brix_semantic::ContextId;
use brix_syntax::ast::{self, BinOp, Expr, Item, Module, Pattern};

pub const PROFILE: &str = "brix.evidence@1";
/// Portable certificate envelope. A decoded artifact is untrusted until
/// [`verify_certificate`] rechecks it for the proposition and context the
/// consumer names.
pub const CERTIFICATE_PROFILE: &str = "brix.evidence.certificate@1";
const DEFAULT_BUDGET: u32 = 10_000;
const SEAL_PREFIX: &str = "brix.evidence.receipt@1";

/// How a satisfied claim was established. These routes are not ordered grades.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authority {
    /// A concrete predicate or an admitted conversion was checked.
    RuntimeCheck,
    /// The bounded predicate fragment discharged the obligation symbolically.
    BoundedFragment,
    /// The proof kernel accepted a certificate for this exact proposition.
    ProofKernel,
}

/// Why an obligation was not satisfied. None of these is a logical refutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    FailedPredicate { predicate: String, at: String },
    Definedness { op: String },
    MissingEvidence,
    UnsupportedFragment { what: String },
    BudgetExhausted,
    CandidateRejected,
    ContextMismatch { expected: String, found: String },
    ClaimMismatch { expected: String, found: String },
    InsufficientAuthority { required: String, found: String },
    NominalMismatch { expected: String, found: String },
    MissingObligation { obligation: String },
    IncompatibleComposition { obligation: String },
    UnresolvedJoin { claim: String },
    AssumptionRequired { assumptions: Vec<String> },
    NonExhaustiveValidate,
    Malformed { what: String },
}

/// The outcome of one obligation. `Violated` means this value failed a concrete
/// predicate. It does not refute the predicate in general, and `Unknown` never
/// authorizes the consumer that required the obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    Satisfied(Box<Receipt>),
    Violated { predicate: String, at: String },
    Unknown { reason: Reason },
}

impl Disposition {
    fn satisfied(receipt: Receipt) -> Self {
        Self::Satisfied(Box::new(receipt))
    }

    /// Proof-profile rejection is never a published refutation.
    pub const fn is_logical_refutation(&self) -> bool {
        false
    }

    pub const fn authorizes(&self) -> bool {
        matches!(self, Disposition::Satisfied(_))
    }

    pub fn authority(&self) -> Option<Authority> {
        match self {
            Disposition::Satisfied(receipt) => Some(receipt.authority),
            _ => None,
        }
    }
}

/// A verifier-issued handle. External code cannot construct one: the seal is
/// private, and only the checker computes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub proposition: String,
    pub subject: String,
    pub context: String,
    pub authority: Authority,
    pub assumptions: Vec<String>,
    /// Present only when [`Authority::ProofKernel`] accepted this proposition.
    pub kernel_certificate: Option<String>,
    seal: String,
}

impl Receipt {
    pub fn authentic(&self) -> bool {
        self.seal == seal_receipt(self)
    }

    fn verified(self) -> Option<Self> {
        self.authentic().then_some(self)
    }
}

fn seal_receipt(receipt: &Receipt) -> String {
    let assumptions = receipt.assumptions.join(",");
    let kernel = receipt.kernel_certificate.as_deref().unwrap_or("");
    let payload = format!(
        "{SEAL_PREFIX}\n{}\n{}\n{}\n{:?}\n{assumptions}\n{kernel}",
        receipt.proposition, receipt.subject, receipt.context, receipt.authority
    );
    Digest::of(Domain::Claim, payload.as_bytes()).to_hex()
}

fn issue(
    proposition: impl Into<String>,
    subject: impl Into<String>,
    context: impl Into<String>,
    authority: Authority,
    assumptions: Vec<String>,
    kernel_certificate: Option<String>,
) -> Receipt {
    let mut receipt = Receipt {
        proposition: proposition.into(),
        subject: subject.into(),
        context: context.into(),
        authority,
        assumptions,
        kernel_certificate,
        seal: String::new(),
    };
    receipt.seal = seal_receipt(&receipt);
    receipt
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedBinding {
    pub path: String,
    pub ty: String,
    pub disposition: Disposition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmBinding {
    pub path: String,
    pub ty: String,
    /// The arm is the one a concrete scrutinee selects. A refined binder exists
    /// only on the `Satisfied` arm, whether or not that arm is the one taken.
    pub reachable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub item: String,
    pub disposition: Disposition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceReport {
    pub opted_in: bool,
    pub ok: bool,
    pub bindings: Vec<CheckedBinding>,
    pub arms: Vec<ArmBinding>,
    pub diagnostics: Vec<Diagnostic>,
}

impl EvidenceReport {
    fn idle() -> Self {
        Self {
            opted_in: false,
            ok: true,
            bindings: Vec::new(),
            arms: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

/// A proof candidate presented to the acceptance boundary. A label is not a
/// certificate. A term is untrusted until the kernel accepts it for the
/// proposition and context the consumer names.
pub enum Candidate<'a> {
    ForgedLabel(&'a str),
    Term {
        term: &'a ExplicitTerm,
        proposition: &'a Prop,
        expected_proposition: &'a Prop,
        expected_context: &'a ContextId,
    },
}

pub fn check_module(module: &Module) -> EvidenceReport {
    check_module_with_budget(module, DEFAULT_BUDGET)
}

pub fn check_module_with_budget(module: &Module, budget: u32) -> EvidenceReport {
    if !module_opts_in(module) {
        return EvidenceReport::idle();
    }
    let mut checker = Checker::new(module, budget);
    checker.run()
}

pub fn module_opts_in(module: &Module) -> bool {
    module.items.iter().any(item_opts_in)
}

/// Drop bindings the evidence profile owns so the ordinary typing checker does
/// not report them as unknown types. Declarations the ordinary checker ignores
/// stay in place. Modules that do not opt in are returned unchanged.
pub fn without_evidence_bindings(module: &Module) -> Module {
    if !module_opts_in(module) {
        return module.clone();
    }
    let names = evidence_type_names(module);
    Module {
        items: module
            .items
            .iter()
            .filter(|item| !binding_owned_by_evidence(item, &names))
            .cloned()
            .collect(),
    }
}

pub fn admit_candidate(candidate: Candidate<'_>, budget: Budget) -> Disposition {
    match candidate {
        Candidate::ForgedLabel(label) => Disposition::Unknown {
            reason: Reason::InsufficientAuthority {
                required: "ProofKernel certificate".to_string(),
                found: format!("forged label {label}"),
            },
        },
        Candidate::Term {
            term,
            proposition,
            expected_proposition,
            expected_context,
        } => {
            if proposition != expected_proposition {
                return Disposition::Unknown {
                    reason: Reason::ClaimMismatch {
                        expected: format!("{expected_proposition:?}"),
                        found: format!("{proposition:?}"),
                    },
                };
            }
            if term.context != *expected_context {
                return Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: expected_context.to_hex(),
                        found: term.context.to_hex(),
                    },
                };
            }
            match acceptance(expected_context, expected_proposition, term, budget) {
                Verdict::Accepted(certificate) => Disposition::satisfied(issue(
                    format!("{expected_proposition:?}"),
                    certificate.certificate_id.to_hex(),
                    expected_context.to_hex(),
                    Authority::ProofKernel,
                    Vec::new(),
                    Some(certificate.certificate_id.to_hex()),
                )),
                Verdict::Rejected(_) => Disposition::Unknown {
                    reason: Reason::CandidateRejected,
                },
                Verdict::ResourceExhausted(_) => Disposition::Unknown {
                    reason: Reason::BudgetExhausted,
                },
                Verdict::ContextMismatch {
                    claimed,
                    term_context,
                } => Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: claimed.to_hex(),
                        found: term_context.to_hex(),
                    },
                },
                Verdict::Unsupported(_) | Verdict::Malformed(_) => Disposition::Unknown {
                    reason: Reason::UnsupportedFragment {
                        what: "proof candidate".to_string(),
                    },
                },
            }
        }
    }
}

/// Direct predicate evaluation for the admitted integer fragment.
///
/// Refinement checking calls this same interpreter. Agreement between the two
/// is a regression check on the disposition mapping, not a kernel proof.
pub fn eval_predicate(predicate: &Expr, binder: &str, value: i64) -> Disposition {
    let mut env = Env::default();
    env.ints
        .insert(binder.to_string(), IntExpr::Concrete(value));
    let mut steps = DEFAULT_BUDGET;
    match decide(&mut steps, &env, predicate, &BTreeMap::new()) {
        Pred::True { assumptions } => Disposition::satisfied(issue(
            render_expr(predicate),
            value.to_string(),
            PROFILE.to_string(),
            Authority::RuntimeCheck,
            assumptions,
            None,
        )),
        Pred::False { .. } => Disposition::Violated {
            predicate: render_expr(predicate),
            at: value.to_string(),
        },
        Pred::Unknown(reason) => Disposition::Unknown { reason },
    }
}

/// Untrusted portable evidence. Constructing or decoding one does not issue a
/// [`Receipt`]. The claimed authority is a label, not a route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableCertificate {
    pub profile: String,
    pub proposition: String,
    pub subject: String,
    pub context: String,
    pub assumptions: Vec<String>,
    pub claimed_authority: Authority,
    pub artifact: CertificateArtifact,
}

/// The two admitted certificate forms.
///
/// Integer predicate: premises are the profile, a `RuntimeCheck` label, and a
/// predicate in the integer fragment. The conclusion is that the named value
/// satisfies that predicate. Checking re-parses the source and evaluates it.
/// A passing check stays on `RuntimeCheck` and carries no kernel certificate.
///
/// Kernel term: premises are the profile, a `ProofKernel` label, and an
/// explicit term. The conclusion is the proposition the consumer names.
/// Checking is `brix_kernel::acceptance` for that proposition and context.
/// Rejection and budget exhaustion stay unresolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertificateArtifact {
    IntegerPredicate {
        binder: String,
        source: String,
        value: i64,
    },
    Kernel {
        term: Box<ExplicitTerm>,
        proposition: Box<Prop>,
    },
}

/// The claim the consumer is willing to accept. The artifact does not choose it.
///
/// `assumptions` are the premises this consumer authorizes. The default is
/// unconditional: an empty list. A certificate cannot enlarge that list.
#[derive(Clone, Copy)]
pub enum ExpectedTarget<'a> {
    Predicate {
        proposition: &'a str,
        context: &'a str,
        binder: &'a str,
        subject: &'a str,
        assumptions: &'a [String],
    },
    Kernel {
        proposition: &'a Prop,
        context: &'a ContextId,
    },
}

/// Build an integer-predicate certificate. The proposition text is the
/// rendered predicate, so a later edit of `source` is a different claim.
pub fn integer_predicate_certificate(
    binder: &str,
    source: &str,
    value: i64,
    context: &str,
    assumptions: Vec<String>,
    claimed_authority: Authority,
) -> Result<PortableCertificate, Disposition> {
    if source.contains('\n') {
        return Err(malformed("predicate source"));
    }
    let predicate = parse_predicate_source(source)?;
    Ok(PortableCertificate {
        profile: CERTIFICATE_PROFILE.to_string(),
        proposition: render_expr(&predicate),
        subject: value.to_string(),
        context: context.to_string(),
        assumptions,
        claimed_authority,
        artifact: CertificateArtifact::IntegerPredicate {
            binder: binder.to_string(),
            source: source.to_string(),
            value,
        },
    })
}

pub fn kernel_certificate(
    term: ExplicitTerm,
    proposition: Prop,
    claimed_authority: Authority,
) -> PortableCertificate {
    let context = term.context.to_hex();
    PortableCertificate {
        profile: CERTIFICATE_PROFILE.to_string(),
        proposition: format!("{proposition:?}"),
        subject: String::new(),
        context,
        assumptions: Vec::new(),
        claimed_authority,
        artifact: CertificateArtifact::Kernel {
            term: Box::new(term),
            proposition: Box::new(proposition),
        },
    }
}

/// Canonical text for an integer-predicate certificate. Kernel terms stay in
/// the explicit-term representation the kernel already checks.
pub fn encode_integer_certificate(
    certificate: &PortableCertificate,
) -> Result<String, Disposition> {
    let CertificateArtifact::IntegerPredicate {
        binder,
        source,
        value,
    } = &certificate.artifact
    else {
        return Err(Disposition::Unknown {
            reason: Reason::UnsupportedFragment {
                what: "integer certificate encoding".to_string(),
            },
        });
    };
    if certificate.profile != CERTIFICATE_PROFILE || source.contains('\n') {
        return Err(malformed("certificate envelope"));
    }
    Ok(format!(
        "{CERTIFICATE_PROFILE}\nproposition={}\nsubject={}\ncontext={}\nassumptions={}\nauthority={}\nkind=integer-predicate\nbinder={binder}\nvalue={value}\npredicate={source}\n",
        certificate.proposition,
        certificate.subject,
        certificate.context,
        certificate.assumptions.join(","),
        authority_label(certificate.claimed_authority),
    ))
}

pub fn decode_integer_certificate(text: &str) -> Result<PortableCertificate, Disposition> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != 10 || lines[0] != CERTIFICATE_PROFILE {
        return Err(malformed("certificate envelope"));
    }
    let kind = certificate_field(lines[6], "kind")?;
    if kind != "integer-predicate" {
        return Err(Disposition::Unknown {
            reason: Reason::UnsupportedFragment {
                what: format!("certificate kind {kind}"),
            },
        });
    }
    let value = certificate_field(lines[8], "value")?
        .parse::<i64>()
        .map_err(|_| malformed("certificate value"))?;
    Ok(PortableCertificate {
        profile: CERTIFICATE_PROFILE.to_string(),
        proposition: certificate_field(lines[1], "proposition")?,
        subject: certificate_field(lines[2], "subject")?,
        context: certificate_field(lines[3], "context")?,
        assumptions: split_assumptions(&certificate_field(lines[4], "assumptions")?),
        claimed_authority: parse_authority(&certificate_field(lines[5], "authority")?)?,
        artifact: CertificateArtifact::IntegerPredicate {
            binder: certificate_field(lines[7], "binder")?,
            source: certificate_field(lines[9], "predicate")?,
            value,
        },
    })
}

pub fn verify_certificate(
    certificate: &PortableCertificate,
    expected: ExpectedTarget<'_>,
    kernel_budget: Budget,
) -> Disposition {
    verify_certificate_with_steps(certificate, expected, kernel_budget, DEFAULT_BUDGET)
}

pub fn verify_certificate_with_steps(
    certificate: &PortableCertificate,
    expected: ExpectedTarget<'_>,
    kernel_budget: Budget,
    predicate_steps: u32,
) -> Disposition {
    if certificate.profile != CERTIFICATE_PROFILE {
        return Disposition::Unknown {
            reason: Reason::UnsupportedFragment {
                what: format!("certificate profile {}", certificate.profile),
            },
        };
    }
    match (&certificate.artifact, expected) {
        (
            CertificateArtifact::IntegerPredicate {
                binder,
                source,
                value,
            },
            ExpectedTarget::Predicate {
                proposition,
                context,
                binder: expected_binder,
                subject,
                assumptions,
            },
        ) => verify_integer_predicate(
            certificate,
            binder,
            source,
            *value,
            ExpectedPredicate {
                proposition,
                context,
                binder: expected_binder,
                subject,
                assumptions,
            },
            predicate_steps,
        ),
        (
            CertificateArtifact::Kernel { term, proposition },
            ExpectedTarget::Kernel {
                proposition: expected_proposition,
                context,
            },
        ) => verify_kernel_term(
            certificate,
            term,
            proposition,
            expected_proposition,
            context,
            kernel_budget,
        ),
        (CertificateArtifact::IntegerPredicate { .. }, ExpectedTarget::Kernel { .. }) => {
            Disposition::Unknown {
                reason: Reason::InsufficientAuthority {
                    required: "ProofKernel certificate".to_string(),
                    found: "integer predicate".to_string(),
                },
            }
        }
        (CertificateArtifact::Kernel { .. }, ExpectedTarget::Predicate { .. }) => {
            Disposition::Unknown {
                reason: Reason::InsufficientAuthority {
                    required: "RuntimeCheck predicate".to_string(),
                    found: "kernel term".to_string(),
                },
            }
        }
    }
}

struct ExpectedPredicate<'a> {
    proposition: &'a str,
    context: &'a str,
    binder: &'a str,
    subject: &'a str,
    assumptions: &'a [String],
}

fn verify_integer_predicate(
    certificate: &PortableCertificate,
    binder: &str,
    source: &str,
    value: i64,
    expected: ExpectedPredicate<'_>,
    predicate_steps: u32,
) -> Disposition {
    let ExpectedPredicate {
        proposition: expected_proposition,
        context: expected_context,
        binder: expected_binder,
        subject: expected_subject,
        assumptions: authorized,
    } = expected;
    if certificate.proposition != expected_proposition {
        return Disposition::Unknown {
            reason: Reason::ClaimMismatch {
                expected: expected_proposition.to_string(),
                found: certificate.proposition.clone(),
            },
        };
    }
    if certificate.context != expected_context {
        return Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: expected_context.to_string(),
                found: certificate.context.clone(),
            },
        };
    }
    if certificate.claimed_authority != Authority::RuntimeCheck {
        return Disposition::Unknown {
            reason: Reason::InsufficientAuthority {
                required: "RuntimeCheck".to_string(),
                found: authority_label(certificate.claimed_authority).to_string(),
            },
        };
    }
    if certificate.subject != expected_subject || value.to_string() != expected_subject {
        return Disposition::Unknown {
            reason: Reason::ClaimMismatch {
                expected: format!("subject {expected_subject}"),
                found: format!("subject {} value {value}", certificate.subject),
            },
        };
    }
    if binder != expected_binder {
        return Disposition::Unknown {
            reason: Reason::ClaimMismatch {
                expected: format!("binder {expected_binder}"),
                found: format!("binder {binder}"),
            },
        };
    }
    let unauthorized: Vec<String> = certificate
        .assumptions
        .iter()
        .filter(|name| !authorized.iter().any(|allowed| allowed == *name))
        .cloned()
        .collect();
    if !unauthorized.is_empty() {
        return Disposition::Unknown {
            reason: Reason::AssumptionRequired {
                assumptions: unauthorized,
            },
        };
    }
    let predicate = match parse_predicate_source(source) {
        Ok(predicate) => predicate,
        Err(disposition) => return disposition,
    };
    let rendered = render_expr(&predicate);
    if rendered != expected_proposition {
        return Disposition::Unknown {
            reason: Reason::ClaimMismatch {
                expected: expected_proposition.to_string(),
                found: rendered,
            },
        };
    }
    let mut env = Env::default();
    env.ints
        .insert(binder.to_string(), IntExpr::Concrete(value));
    env.assumptions = authorized.iter().cloned().collect();
    let mut steps = predicate_steps;
    match decide(&mut steps, &env, &predicate, &BTreeMap::new()) {
        Pred::True { assumptions } => {
            if assumptions
                .iter()
                .any(|name| !certificate.assumptions.contains(name))
            {
                return Disposition::Unknown {
                    reason: Reason::AssumptionRequired {
                        assumptions: assumptions.clone(),
                    },
                };
            }
            Disposition::satisfied(issue(
                expected_proposition.to_string(),
                value.to_string(),
                expected_context.to_string(),
                Authority::RuntimeCheck,
                assumptions,
                None,
            ))
        }
        Pred::False { .. } => Disposition::Violated {
            predicate: rendered,
            at: value.to_string(),
        },
        Pred::Unknown(reason) => Disposition::Unknown { reason },
    }
}

fn verify_kernel_term(
    certificate: &PortableCertificate,
    term: &ExplicitTerm,
    proposition: &Prop,
    expected_proposition: &Prop,
    expected_context: &ContextId,
    budget: Budget,
) -> Disposition {
    if certificate.claimed_authority != Authority::ProofKernel {
        return Disposition::Unknown {
            reason: Reason::InsufficientAuthority {
                required: "ProofKernel".to_string(),
                found: authority_label(certificate.claimed_authority).to_string(),
            },
        };
    }
    let expected_text = format!("{expected_proposition:?}");
    if certificate.proposition != expected_text || proposition != expected_proposition {
        return Disposition::Unknown {
            reason: Reason::ClaimMismatch {
                expected: expected_text,
                found: certificate.proposition.clone(),
            },
        };
    }
    let expected_context_text = expected_context.to_hex();
    if certificate.context != expected_context_text || term.context != *expected_context {
        return Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: expected_context_text,
                found: certificate.context.clone(),
            },
        };
    }
    match acceptance(expected_context, expected_proposition, term, budget) {
        Verdict::Accepted(accepted) => Disposition::satisfied(issue(
            expected_text,
            accepted.certificate_id.to_hex(),
            expected_context_text,
            Authority::ProofKernel,
            Vec::new(),
            Some(accepted.certificate_id.to_hex()),
        )),
        Verdict::Rejected(_) => Disposition::Unknown {
            reason: Reason::CandidateRejected,
        },
        Verdict::ResourceExhausted(_) => Disposition::Unknown {
            reason: Reason::BudgetExhausted,
        },
        Verdict::ContextMismatch {
            claimed,
            term_context,
        } => Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: claimed.to_hex(),
                found: term_context.to_hex(),
            },
        },
        Verdict::Unsupported(_) | Verdict::Malformed(_) => Disposition::Unknown {
            reason: Reason::UnsupportedFragment {
                what: "proof candidate".to_string(),
            },
        },
    }
}

fn parse_predicate_source(source: &str) -> Result<Expr, Disposition> {
    let wrapped = format!("let v = {source}\n");
    let module = brix_syntax::parse(&wrapped).map_err(|err| malformed(err.to_string()))?;
    match module.items.first() {
        Some(Item::Let(binding)) => Ok(binding.value.clone()),
        _ => Err(malformed("predicate source")),
    }
}

fn certificate_field(line: &str, key: &str) -> Result<String, Disposition> {
    let Some((found, value)) = line.split_once('=') else {
        return Err(malformed("certificate field"));
    };
    if found != key {
        return Err(malformed(format!("certificate field {key}")));
    }
    Ok(value.to_string())
}

fn split_assumptions(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split(',').map(str::to_string).collect()
    }
}

fn authority_label(authority: Authority) -> &'static str {
    match authority {
        Authority::RuntimeCheck => "RuntimeCheck",
        Authority::BoundedFragment => "BoundedFragment",
        Authority::ProofKernel => "ProofKernel",
    }
}

fn parse_authority(text: &str) -> Result<Authority, Disposition> {
    match text {
        "RuntimeCheck" => Ok(Authority::RuntimeCheck),
        "BoundedFragment" => Ok(Authority::BoundedFragment),
        "ProofKernel" => Ok(Authority::ProofKernel),
        _ => Err(malformed(format!("authority {text}"))),
    }
}

fn malformed(what: impl Into<String>) -> Disposition {
    Disposition::Unknown {
        reason: Reason::Malformed { what: what.into() },
    }
}

/// One order's quantity at one revision. The quantity is the dependency a
/// dispatch receipt is about; other orders are unrelated state.
///
/// `world` is an opaque revision digest supplied by the caller. Recording it
/// does not validate the context it names. Digest equality is not confinement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub revision: u64,
    pub world: String,
    pub quantities: BTreeMap<String, i64>,
}

/// Journal schema for retained policy snapshots and the claims made about them.
///
/// The admitted policy is `qty >= 1` on one named order. A checker reads that
/// quantity from the snapshot and evaluates the comparison again. The claims
/// list is retained history, not an applicability cache. A later revision does
/// not delete an earlier claim and does not turn it into a refutation.
/// SOC-LAW-06 stays open: this journal does not validate context contents.
/// SOC-LAW-09 stays partial: old claims remain attributable to their revision.
pub const JOURNAL_PROFILE: &str = "brix.evidence.journal@1";

/// The only policy this journal evaluates.
pub const DISPATCH_POLICY: &str = "qty>=1";

#[derive(Clone, Debug, Default)]
pub struct History {
    snapshots: BTreeMap<u64, Snapshot>,
    claims: Vec<Receipt>,
}

impl History {
    /// Record `snapshot`. A revision already stored with different contents is
    /// left unchanged: the retained claim keeps the snapshot it was checked
    /// against.
    pub fn record_snapshot(&mut self, snapshot: Snapshot) -> Result<(), Disposition> {
        if let Some(existing) = self.snapshots.get(&snapshot.revision) {
            if existing != &snapshot {
                return Err(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!(
                            "rev={} world={}",
                            existing.revision,
                            world_label(&existing.world)
                        ),
                        found: format!(
                            "rev={} world={}",
                            snapshot.revision,
                            world_label(&snapshot.world)
                        ),
                    },
                });
            }
            return Ok(());
        }
        self.snapshots.insert(snapshot.revision, snapshot);
        Ok(())
    }

    pub fn claims(&self) -> &[Receipt] {
        &self.claims
    }

    /// Check `quantity >= 1` for `order` at `revision` and store the receipt.
    /// A later edit does not delete it.
    pub fn establish_can_dispatch(&mut self, revision: u64, order: &str) -> Disposition {
        let Some(snapshot) = self.snapshots.get(&revision) else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        let Some(qty) = snapshot.quantities.get(order).copied() else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        if qty < 1 {
            return Disposition::Violated {
                predicate: "qty >= 1".to_string(),
                at: qty.to_string(),
            };
        }
        let receipt = issue(
            format!("CanDispatch({order})"),
            order.to_string(),
            dispatch_context(revision, &snapshot.world, order, qty),
            Authority::RuntimeCheck,
            Vec::new(),
            None,
        );
        self.claims.push(receipt.clone());
        Disposition::satisfied(receipt)
    }

    /// Use a receipt for `order` against a current snapshot.
    ///
    /// The receipt stays a historical claim either way. Applicability follows
    /// the recorded quantity dependency, not the revision number alone, so an
    /// unrelated order can change without invalidating this one.
    pub fn dispatch(&self, current: &Snapshot, order: &str, evidence: &Receipt) -> Disposition {
        let Some(evidence) = evidence.clone().verified() else {
            return Disposition::Unknown {
                reason: Reason::Malformed {
                    what: "receipt seal".to_string(),
                },
            };
        };
        if evidence.authority == Authority::ProofKernel {
            return Disposition::Unknown {
                reason: Reason::InsufficientAuthority {
                    required: "RuntimeCheck of qty >= 1".to_string(),
                    found: "ProofKernel".to_string(),
                },
            };
        }
        let expected = format!("CanDispatch({order})");
        if evidence.proposition != expected || evidence.subject != order {
            return Disposition::Unknown {
                reason: Reason::ClaimMismatch {
                    expected,
                    found: format!("{} subject {}", evidence.proposition, evidence.subject),
                },
            };
        }
        let Some(claimed_qty) = context_qty(&evidence.context) else {
            return Disposition::Unknown {
                reason: Reason::Malformed {
                    what: "dispatch context".to_string(),
                },
            };
        };
        let Some(current_qty) = current.quantities.get(order).copied() else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        if current_qty != claimed_qty {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: format!("qty={claimed_qty}"),
                    found: format!("qty={current_qty}"),
                },
            };
        }
        if current_qty < 1 {
            return Disposition::Violated {
                predicate: "qty >= 1".to_string(),
                at: current_qty.to_string(),
            };
        }
        Disposition::satisfied(evidence)
    }

    /// Recompute the historical claim from the stored snapshot.
    ///
    /// The receipt seal and authority label are not consulted. A decoded claim
    /// stays untrusted data until this check reads the snapshot named by the
    /// claim's revision and world, evaluates `qty >= 1`, and issues a fresh
    /// runtime-check receipt. A different world is not renamed onto the result.
    pub fn recheck_historical(&self, evidence: &Receipt) -> Disposition {
        let expected = format!("CanDispatch({})", evidence.subject);
        if evidence.proposition != expected {
            return Disposition::Unknown {
                reason: Reason::ClaimMismatch {
                    expected,
                    found: evidence.proposition.clone(),
                },
            };
        }
        if context_field(&evidence.context, "order").is_some_and(|order| order != evidence.subject)
        {
            return Disposition::Unknown {
                reason: Reason::ClaimMismatch {
                    expected: evidence.subject.clone(),
                    found: context_field(&evidence.context, "order").unwrap_or_default(),
                },
            };
        }
        let Some(certificate) = certificate_from_claim(&evidence.subject, evidence) else {
            return malformed("dispatch context");
        };
        self.recheck_dispatch_certificate(&certificate)
    }

    /// The retained claim for `order`, as data with no receipt seal.
    pub fn export_dispatch(&self, order: &str) -> Option<DispatchCertificate> {
        let receipt = self.claims.iter().find(|claim| {
            claim.subject == order && claim.proposition == format!("CanDispatch({order})")
        })?;
        certificate_from_claim(order, receipt)
    }

    /// Recompute `qty >= 1` from the snapshot named by the certificate.
    /// The certificate's quantity is a claim about that snapshot. It is not
    /// an applicability cache, and a seal is not consulted.
    pub fn recheck_dispatch_certificate(&self, certificate: &DispatchCertificate) -> Disposition {
        if certificate.policy != DISPATCH_POLICY {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: DISPATCH_POLICY.to_string(),
                    found: certificate.policy.clone(),
                },
            };
        }
        let Some(snapshot) = self.snapshots.get(&certificate.revision) else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        let Some(stored) = snapshot.quantities.get(&certificate.order).copied() else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        if snapshot.world != certificate.world {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: format!("world={}", world_label(&certificate.world)),
                    found: format!("world={}", world_label(&snapshot.world)),
                },
            };
        }
        if stored != certificate.quantity {
            return Disposition::Unknown {
                reason: Reason::Malformed {
                    what: format!(
                        "snapshot qty {stored} disagrees with certificate qty {}",
                        certificate.quantity
                    ),
                },
            };
        }
        if stored < 1 {
            return Disposition::Violated {
                predicate: "qty >= 1".to_string(),
                at: stored.to_string(),
            };
        }
        Disposition::satisfied(issue(
            format!("CanDispatch({})", certificate.order),
            certificate.order.clone(),
            dispatch_context(
                certificate.revision,
                &snapshot.world,
                &certificate.order,
                stored,
            ),
            Authority::RuntimeCheck,
            Vec::new(),
            None,
        ))
    }

    /// The retained claim for `order` at one revision.
    pub fn export_dispatch_at(&self, order: &str, revision: u64) -> Option<DispatchCertificate> {
        let receipt = self.claims.iter().find(|claim| {
            claim.subject == order
                && claim.proposition == format!("CanDispatch({order})")
                && context_revision(&claim.context) == Some(revision)
        })?;
        certificate_from_claim(order, receipt)
    }

    /// Use `certificate` for this order, policy, and revision.
    ///
    /// The revision is the one the certificate names. A later snapshot, a
    /// different order, or a different policy is a different claim. Checking
    /// recomputes `qty >= 1` from the stored snapshot.
    pub fn use_dispatch(
        &self,
        certificate: &DispatchCertificate,
        order: &str,
        policy: &str,
        revision: u64,
    ) -> Disposition {
        if policy != DISPATCH_POLICY {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: DISPATCH_POLICY.to_string(),
                    found: policy.to_string(),
                },
            };
        }
        if certificate.policy != DISPATCH_POLICY {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: DISPATCH_POLICY.to_string(),
                    found: certificate.policy.clone(),
                },
            };
        }
        if certificate.order != order {
            return Disposition::Unknown {
                reason: Reason::ClaimMismatch {
                    expected: format!("CanDispatch({order})"),
                    found: format!("CanDispatch({})", certificate.order),
                },
            };
        }
        if certificate.revision != revision {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: format!("rev={revision}"),
                    found: format!("rev={}", certificate.revision),
                },
            };
        }
        self.recheck_dispatch_certificate(certificate)
    }

    /// Carry a checked claim onto a later revision when that order's quantity
    /// is unchanged. The original claim stays in the journal.
    pub fn transfer_dispatch(
        &mut self,
        certificate: &DispatchCertificate,
        revision: u64,
    ) -> Disposition {
        let historical = self.recheck_dispatch_certificate(certificate);
        if !historical.authorizes() {
            return historical;
        }
        let Some(snapshot) = self.snapshots.get(&revision) else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        let Some(qty) = snapshot.quantities.get(&certificate.order).copied() else {
            return Disposition::Unknown {
                reason: Reason::MissingEvidence,
            };
        };
        if qty != certificate.quantity {
            return Disposition::Unknown {
                reason: Reason::ContextMismatch {
                    expected: format!("qty={}", certificate.quantity),
                    found: format!("qty={qty}"),
                },
            };
        }
        self.establish_can_dispatch(revision, &certificate.order)
    }

    /// Canonical text for the snapshots and the claims retained about them.
    pub fn encode_journal(&self) -> Result<String, Disposition> {
        let mut out = String::from(JOURNAL_PROFILE);
        out.push('\n');
        for snapshot in self.snapshots.values() {
            if !journal_token(&snapshot.world) && !snapshot.world.is_empty() {
                return Err(malformed("world digest"));
            }
            let world = if snapshot.world.is_empty() {
                "-"
            } else {
                snapshot.world.as_str()
            };
            out.push_str(&format!("snapshot {} {world}\n", snapshot.revision));
            for (order, qty) in &snapshot.quantities {
                if !journal_token(order) {
                    return Err(malformed("order name"));
                }
                out.push_str(&format!("{order}={qty}\n"));
            }
            out.push_str(".\n");
        }
        for claim in &self.claims {
            if claim.proposition.contains('\n') || claim.context.contains('\n') {
                return Err(malformed("claim"));
            }
            out.push_str("claim\n");
            out.push_str(&format!("proposition={}\n", claim.proposition));
            out.push_str(&format!("subject={}\n", claim.subject));
            out.push_str(&format!("context={}\n", claim.context));
            out.push_str(&format!("authority={}\n", authority_label(claim.authority)));
            out.push_str(&format!("assumptions={}\n", claim.assumptions.join(",")));
            out.push_str(".\n");
        }
        Ok(out)
    }

    /// Load a journal. Seals are recomputed from the recorded claim fields.
    /// Applicability is not stored in the text.
    pub fn decode_journal(text: &str) -> Result<Self, Disposition> {
        let mut lines = text.lines();
        if lines.next() != Some(JOURNAL_PROFILE) {
            return Err(malformed("journal profile"));
        }
        let mut history = History::default();
        let lines: Vec<&str> = lines.collect();
        let mut index = 0;
        while index < lines.len() {
            if let Some(rest) = lines[index].strip_prefix("snapshot ") {
                let mut parts = rest.split(' ');
                let revision = parts
                    .next()
                    .ok_or_else(|| malformed("snapshot revision"))?
                    .parse::<u64>()
                    .map_err(|_| malformed("snapshot revision"))?;
                let world = parts.next().ok_or_else(|| malformed("snapshot world"))?;
                if parts.next().is_some() || (!journal_token(world) && world != "-") {
                    return Err(malformed("snapshot world"));
                }
                index += 1;
                let mut quantities = BTreeMap::new();
                while index < lines.len() && lines[index] != "." {
                    let (order, qty) = lines[index]
                        .split_once('=')
                        .ok_or_else(|| malformed("snapshot quantity"))?;
                    if !journal_token(order) {
                        return Err(malformed("order name"));
                    }
                    let qty = qty
                        .parse::<i64>()
                        .map_err(|_| malformed("snapshot quantity"))?;
                    quantities.insert(order.to_string(), qty);
                    index += 1;
                }
                if index >= lines.len() || lines[index] != "." {
                    return Err(malformed("snapshot end"));
                }
                history.record_snapshot(Snapshot {
                    revision,
                    world: world_id(world),
                    quantities,
                })?;
                index += 1;
            } else if lines[index] == "claim" {
                index += 1;
                let proposition = claim_field(lines.get(index), "proposition")?;
                let subject = claim_field(lines.get(index + 1), "subject")?;
                let context = claim_field(lines.get(index + 2), "context")?;
                let authority = parse_authority(&claim_field(lines.get(index + 3), "authority")?)?;
                let assumptions =
                    split_assumptions(&claim_field(lines.get(index + 4), "assumptions")?);
                index += 5;
                if lines.get(index) != Some(&".") {
                    return Err(malformed("claim end"));
                }
                // The authority field is a label on untrusted bytes. Leave the
                // seal unset so the claim is not an authentic receipt. A later
                // check issues authority only after the snapshot confirms it,
                // and a kernel label is never that authority.
                history.claims.push(Receipt {
                    proposition,
                    subject,
                    context,
                    authority,
                    assumptions,
                    kernel_certificate: None,
                    seal: String::new(),
                });
                index += 1;
            } else {
                return Err(malformed("journal entry"));
            }
        }
        Ok(history)
    }
}

/// Check a portable certificate against a persisted journal.
///
/// The journal text is the only state. A cached authorization bit is not
/// part of the text, and the claims section is not consulted: the snapshot
/// named by the certificate is read and `qty >= 1` is evaluated again.
pub fn check_persisted_dispatch(
    journal: &str,
    certificate: &DispatchCertificate,
    order: &str,
    policy: &str,
    revision: u64,
) -> Disposition {
    match History::decode_journal(journal) {
        Ok(history) => history.use_dispatch(certificate, order, policy, revision),
        Err(disposition) => disposition,
    }
}

fn certificate_from_claim(order: &str, receipt: &Receipt) -> Option<DispatchCertificate> {
    Some(DispatchCertificate {
        order: order.to_string(),
        policy: context_field(&receipt.context, "policy").unwrap_or_else(|| DISPATCH_POLICY.into()),
        revision: context_revision(&receipt.context)?,
        world: world_id(
            &context_field(&receipt.context, "world").unwrap_or_else(|| "-".to_string()),
        ),
        quantity: context_qty(&receipt.context)?,
    })
}

fn world_label(world: &str) -> &str {
    if world.is_empty() {
        "-"
    } else {
        world
    }
}

fn world_id(label: &str) -> String {
    if label == "-" {
        String::new()
    } else {
        label.to_string()
    }
}

fn journal_token(value: &str) -> bool {
    !value.is_empty() && !value.contains(['\n', ';', '=', ' '])
}

fn claim_field(line: Option<&&str>, key: &str) -> Result<String, Disposition> {
    let line = line.ok_or_else(|| malformed("claim field"))?;
    let (found, value) = line
        .split_once('=')
        .ok_or_else(|| malformed("claim field"))?;
    if found != key {
        return Err(malformed("claim field"));
    }
    Ok(value.to_string())
}

/// A dispatch claim that can be checked from a retained snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchCertificate {
    pub order: String,
    pub policy: String,
    pub revision: u64,
    /// World identity the certificate was checked against. Ordinary use
    /// requires this same identity. A different world is a transfer.
    pub world: String,
    pub quantity: i64,
}

fn dispatch_context(revision: u64, world: &str, order: &str, qty: i64) -> String {
    format!(
        "rev={revision};world={};policy={DISPATCH_POLICY};order={order};qty={qty}",
        world_label(world)
    )
}

fn context_field(context: &str, key: &str) -> Option<String> {
    context.split(';').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn context_qty(context: &str) -> Option<i64> {
    context_field(context, "qty")?.parse().ok()
}

fn context_revision(context: &str) -> Option<u64> {
    context_field(context, "rev")?.parse().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum IntExpr {
    Concrete(i64),
    Sym(String),
    Add(Box<IntExpr>, Box<IntExpr>),
    Sub(Box<IntExpr>, Box<IntExpr>),
}

#[derive(Clone, Debug)]
struct Value {
    ty: SemTy,
    int: Option<IntExpr>,
    bool: Option<bool>,
    /// Predicates of `it` known about this value.
    facts: Vec<Expr>,
    assumptions: Vec<String>,
    receipt: Option<Receipt>,
    conversion: Option<Conversion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Conversion {
    rule: String,
    from: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SemTy {
    Int,
    Bool,
    Nominal {
        name: String,
        indices: Vec<SemIndex>,
    },
    Refine {
        name: Option<String>,
        binder: String,
        predicate: Expr,
        base: Box<SemTy>,
    },
    /// Result of `validate`. This is not the refined type. The refined value
    /// is bound only by the `Satisfied` arm of a match on this outcome.
    Validation {
        refined: Box<SemTy>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SemIndex {
    Tag(String),
    Int(i64),
}

impl SemTy {
    fn render(&self) -> String {
        match self {
            SemTy::Int => "Int".to_string(),
            SemTy::Bool => "Bool".to_string(),
            SemTy::Nominal { name, indices } => {
                if indices.is_empty() {
                    name.clone()
                } else {
                    let args = indices
                        .iter()
                        .map(|index| match index {
                            SemIndex::Tag(tag) => tag.clone(),
                            SemIndex::Int(n) => n.to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{name}<{args}>")
                }
            }
            SemTy::Refine {
                name: Some(name), ..
            } => name.clone(),
            SemTy::Refine { predicate, .. } => format!("{{ _ | {} }}", render_expr(predicate)),
            SemTy::Validation { refined } => format!("validate<{}>", refined.render()),
        }
    }
}

#[derive(Clone, Debug)]
enum Declared {
    Newtype {
        params: Vec<String>,
    },
    Alias {
        params: Vec<(String, bool)>,
        body: SemTy,
    },
}

#[derive(Clone, Debug)]
struct FunSig {
    type_params: Vec<String>,
    params: Vec<(String, SemTy)>,
    ret: Option<SemTy>,
    requires: Option<Expr>,
    ensures: Option<Expr>,
    converts: Option<String>,
    body: Expr,
}

#[derive(Clone, Debug)]
enum Pred {
    True {
        assumptions: Vec<String>,
    },
    /// False under `assumptions`. An empty list is unconditional. Negation
    /// swaps the two outcomes and keeps the premises that established them,
    /// so `!!a` still depends on `a`.
    False {
        assumptions: Vec<String>,
    },
    Unknown(Reason),
}

#[derive(Clone, Default)]
struct Env {
    ints: BTreeMap<String, IntExpr>,
    bools: BTreeMap<String, bool>,
    values: BTreeMap<String, Value>,
    assumptions: BTreeSet<String>,
}

struct Checker<'a> {
    module: &'a Module,
    steps: u32,
    decls: BTreeMap<String, Declared>,
    funs: BTreeMap<String, FunSig>,
    checked_funs: BTreeSet<String>,
    checking_funs: BTreeSet<String>,
    env: Env,
    report: EvidenceReport,
}

impl<'a> Checker<'a> {
    fn new(module: &'a Module, budget: u32) -> Self {
        let mut assumptions = BTreeSet::new();
        let mut funs = BTreeMap::new();
        let mut decls = BTreeMap::new();
        for item in &module.items {
            match item {
                Item::Assume(name) => {
                    assumptions.insert(name.clone());
                }
                Item::Fn(fun) | Item::Rule(fun) => {
                    funs.insert(fun.name.clone(), fun_placeholder(fun));
                }
                _ => {}
            }
        }
        let _ = (&mut decls, &funs);
        Self {
            module,
            steps: budget,
            decls,
            funs,
            checked_funs: BTreeSet::new(),
            checking_funs: BTreeSet::new(),
            env: Env {
                assumptions,
                ..Env::default()
            },
            report: EvidenceReport {
                opted_in: true,
                ok: true,
                bindings: Vec::new(),
                arms: Vec::new(),
                diagnostics: Vec::new(),
            },
        }
    }

    fn run(&mut self) -> EvidenceReport {
        for item in &self.module.items {
            match item {
                Item::Newtype(decl) => {
                    if !matches!(decl.repr, ast::Ty::Named(ref n) if n == "Int") {
                        self.fail(
                            &decl.name,
                            Disposition::Unknown {
                                reason: Reason::UnsupportedFragment {
                                    what: "newtype representation other than Int".to_string(),
                                },
                            },
                        );
                        continue;
                    }
                    self.decls.insert(
                        decl.name.clone(),
                        Declared::Newtype {
                            params: decl.params.clone(),
                        },
                    );
                }
                Item::TypeDef(decl) => {
                    match self.resolve_ty(&decl.body, &index_env(&decl.params)) {
                        Ok(body) => {
                            let params = decl
                            .params
                            .iter()
                            .map(|param| {
                                (
                                    param.name.clone(),
                                    matches!(param.sort, Some(ast::Ty::Named(ref n)) if n == "Int"),
                                )
                            })
                            .collect();
                            self.decls
                                .insert(decl.name.clone(), Declared::Alias { params, body });
                        }
                        Err(disposition) => self.fail(&decl.name, disposition),
                    }
                }
                Item::Fn(fun) | Item::Rule(fun) => {
                    if let Err(disposition) = self.prepare_fun(fun) {
                        self.fail(&fun.name, disposition);
                    }
                }
                _ => {}
            }
        }
        let names: Vec<String> = self.funs.keys().cloned().collect();
        for name in names {
            if let Err(disposition) = self.ensure_fun(&name) {
                self.fail(&name, disposition);
            }
        }
        for item in &self.module.items {
            if let Item::Let(binding) = item {
                match self.eval_let(binding) {
                    Ok(()) => {}
                    Err(disposition) => self.fail(&binding.name, disposition),
                }
            }
        }
        self.report.clone()
    }

    fn fail(&mut self, item: &str, disposition: Disposition) {
        self.report.ok = false;
        self.report.diagnostics.push(Diagnostic {
            item: item.to_string(),
            disposition,
        });
    }

    fn charge(&mut self) -> Result<(), Disposition> {
        if self.steps == 0 {
            return Err(Disposition::Unknown {
                reason: Reason::BudgetExhausted,
            });
        }
        self.steps -= 1;
        Ok(())
    }

    fn prepare_fun(&mut self, fun: &ast::Callable) -> Result<(), Disposition> {
        let mut params = Vec::new();
        for param in &fun.params {
            let ty = match &param.ty {
                Some(ty) => self.resolve_ty(ty, &BTreeMap::new())?,
                None => SemTy::Int,
            };
            params.push((param.name.clone(), ty));
        }
        let ret = match &fun.ret {
            Some(ty) => Some(self.resolve_ty(ty, &BTreeMap::new())?),
            None => None,
        };
        self.funs.insert(
            fun.name.clone(),
            FunSig {
                type_params: fun.type_params.clone(),
                params,
                ret,
                requires: fun.requires.clone(),
                ensures: fun.ensures.clone(),
                converts: fun.converts.clone(),
                body: fun.body.clone(),
            },
        );
        Ok(())
    }

    fn ensure_fun(&mut self, name: &str) -> Result<(), Disposition> {
        if self.checked_funs.contains(name) {
            return Ok(());
        }
        if !self.checking_funs.insert(name.to_string()) {
            return Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: format!("recursive function {name}"),
                },
            });
        }
        let fun = self.funs.get(name).cloned().ok_or(Disposition::Unknown {
            reason: Reason::MissingEvidence,
        })?;
        let result = self.check_fun(name, &fun);
        self.checking_funs.remove(name);
        if result.is_ok() {
            self.checked_funs.insert(name.to_string());
        }
        result
    }

    fn check_fun(&mut self, name: &str, fun: &FunSig) -> Result<(), Disposition> {
        let mut env = Env {
            assumptions: self.env.assumptions.clone(),
            ..Env::default()
        };
        let mut bounds = BTreeMap::new();
        for (param, ty) in &fun.params {
            env.ints.insert(param.clone(), IntExpr::Sym(param.clone()));
            env.values.insert(param.clone(), symbolic_param(param, ty));
            if let SemTy::Refine { predicate, .. } = ty {
                let fact = subst_var(predicate, ty_binder(ty), param);
                tighten(&fact, &env, &mut bounds);
            }
        }
        let mut contract_assumptions = Vec::new();
        if let Some(requires) = &fun.requires {
            tighten(requires, &env, &mut bounds);
            match decide(&mut self.steps, &env, requires, &bounds) {
                Pred::False { .. } => {
                    return Err(Disposition::Violated {
                        predicate: render_expr(requires),
                        at: "requires".to_string(),
                    });
                }
                Pred::Unknown(Reason::Definedness { op }) => {
                    return Err(Disposition::Unknown {
                        reason: Reason::Definedness { op },
                    });
                }
                Pred::True { assumptions } => {
                    contract_assumptions = union_assumptions(contract_assumptions, assumptions);
                }
                Pred::Unknown(_) => {}
            }
        }
        let body = self.eval_expr(&fun.body, &env, &bounds, name)?;
        contract_assumptions =
            union_assumptions(contract_assumptions, body.assumptions.iter().cloned());
        if let Some(ensures) = &fun.ensures {
            if let Expr::Match {
                scrutinee, arms, ..
            } = &fun.body
            {
                let scrut = self.eval_expr(scrutinee, &env, &bounds, name)?;
                for arm in arms {
                    if !arm_reachable(&arm.pattern, &scrut) {
                        continue;
                    }
                    let mut arm_env = env.clone();
                    bind_pattern(&arm.pattern, &scrut, &mut arm_env);
                    let arm_value = self.eval_expr(&arm.body, &arm_env, &bounds, name)?;
                    let used =
                        self.require_ensures(name, ensures, &arm_value, &arm_env, &bounds)?;
                    contract_assumptions = union_assumptions(contract_assumptions, used);
                }
            } else {
                let used = self.require_ensures(name, ensures, &body, &env, &bounds)?;
                contract_assumptions = union_assumptions(contract_assumptions, used);
            }
        }
        if let Some(ret) = &fun.ret {
            let produced = self.assign(ret, &body, &contract_assumptions, false, &bounds)?;
            contract_assumptions = union_assumptions(contract_assumptions, produced.assumptions);
        }
        let receipt = issue(
            fun.ensures
                .as_ref()
                .map(render_expr)
                .unwrap_or_else(|| format!("{name} contract")),
            name.to_string(),
            PROFILE.to_string(),
            Authority::BoundedFragment,
            contract_assumptions,
            None,
        );
        self.report.bindings.push(CheckedBinding {
            path: name.to_string(),
            ty: fun
                .ret
                .as_ref()
                .map(SemTy::render)
                .unwrap_or_else(|| "Int".to_string()),
            disposition: Disposition::satisfied(receipt),
        });
        Ok(())
    }

    fn eval_let(&mut self, binding: &ast::LetDecl) -> Result<(), Disposition> {
        let (ty, given, demand_kernel) = match &binding.ty {
            Some(ty) => {
                let (inner, given) = split_given(ty);
                let demand_kernel = demanded_kernel(ty);
                (
                    Some(self.resolve_ty(inner, &BTreeMap::new())?),
                    given,
                    demand_kernel,
                )
            }
            None => (None, Vec::new(), false),
        };
        let mut bounds = BTreeMap::new();
        let env = self.env.clone();
        let value = self.eval_expr(&binding.value, &env, &bounds, &binding.name)?;
        let value = if let Some(ty) = &ty {
            self.assign(ty, &value, &given, demand_kernel, &bounds)?
        } else {
            value
        };
        let _ = &mut bounds;
        let disposition = value
            .receipt
            .clone()
            .map(Disposition::satisfied)
            .unwrap_or_else(|| {
                Disposition::satisfied(issue(
                    ty.as_ref()
                        .map(SemTy::render)
                        .unwrap_or_else(|| value.ty.render()),
                    binding.name.clone(),
                    PROFILE.to_string(),
                    Authority::RuntimeCheck,
                    value.assumptions.clone(),
                    None,
                ))
            });
        if let Disposition::Satisfied(receipt) = &disposition {
            if !receipt.assumptions.iter().all(|name| given.contains(name)) {
                return Err(Disposition::Unknown {
                    reason: Reason::AssumptionRequired {
                        assumptions: receipt.assumptions.clone(),
                    },
                });
            }
        }
        self.env.values.insert(binding.name.clone(), value.clone());
        if let Some(int) = &value.int {
            self.env.ints.insert(binding.name.clone(), int.clone());
        }
        self.report.bindings.push(CheckedBinding {
            path: binding.name.clone(),
            ty: value.ty.render(),
            disposition,
        });
        Ok(())
    }

    fn assign(
        &mut self,
        target: &SemTy,
        value: &Value,
        given: &[String],
        demand_kernel: bool,
        bounds: &BTreeMap<String, (i64, i64)>,
    ) -> Result<Value, Disposition> {
        let found_authority = value
            .receipt
            .as_ref()
            .map(|receipt| format!("{:?}", receipt.authority))
            .unwrap_or_else(|| "no certificate".to_string());
        match target {
            SemTy::Int => {
                if demand_kernel {
                    return Err(Disposition::Unknown {
                        reason: Reason::InsufficientAuthority {
                            required: "ProofKernel".to_string(),
                            found: found_authority,
                        },
                    });
                }
                if value.int.is_none() && !matches!(value.ty, SemTy::Int | SemTy::Refine { .. }) {
                    return Err(Disposition::Unknown {
                        reason: Reason::NominalMismatch {
                            expected: "Int".to_string(),
                            found: value.ty.render(),
                        },
                    });
                }
                Ok(Value {
                    ty: SemTy::Int,
                    int: value.int.clone(),
                    bool: None,
                    facts: Vec::new(),
                    assumptions: value.assumptions.clone(),
                    receipt: None,
                    conversion: None,
                })
            }
            SemTy::Bool => {
                if demand_kernel {
                    return Err(Disposition::Unknown {
                        reason: Reason::InsufficientAuthority {
                            required: "ProofKernel".to_string(),
                            found: found_authority,
                        },
                    });
                }
                if value.ty != SemTy::Bool {
                    return Err(Disposition::Unknown {
                        reason: Reason::NominalMismatch {
                            expected: "Bool".to_string(),
                            found: value.ty.render(),
                        },
                    });
                }
                Ok(value.clone())
            }
            SemTy::Nominal { name, indices } => {
                if demand_kernel {
                    return Err(Disposition::Unknown {
                        reason: Reason::InsufficientAuthority {
                            required: "ProofKernel".to_string(),
                            found: found_authority.clone(),
                        },
                    });
                }
                if let Some(conversion) = &value.conversion {
                    if conversion.rule != "identity" {
                        return Err(Disposition::Unknown {
                            reason: Reason::UnsupportedFragment {
                                what: format!("conversion rule {}", conversion.rule),
                            },
                        });
                    }
                    let assumptions = value.assumptions.clone();
                    if !assumptions.iter().all(|name| given.contains(name)) {
                        return Err(Disposition::Unknown {
                            reason: Reason::AssumptionRequired {
                                assumptions: assumptions.clone(),
                            },
                        });
                    }
                    let receipt = issue(
                        format!(
                            "convert(identity): {} -> {}",
                            conversion.from,
                            target.render()
                        ),
                        conversion.from.clone(),
                        PROFILE.to_string(),
                        Authority::RuntimeCheck,
                        assumptions.clone(),
                        None,
                    );
                    return Ok(Value {
                        ty: target.clone(),
                        int: value.int.clone(),
                        bool: None,
                        facts: Vec::new(),
                        assumptions,
                        receipt: Some(receipt),
                        conversion: None,
                    });
                }
                if value.ty
                    != (SemTy::Nominal {
                        name: name.clone(),
                        indices: indices.clone(),
                    })
                {
                    return Err(Disposition::Unknown {
                        reason: Reason::NominalMismatch {
                            expected: target.render(),
                            found: value.ty.render(),
                        },
                    });
                }
                Ok(value.clone())
            }
            SemTy::Validation { .. } => Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: "validation outcome must be matched".to_string(),
                },
            }),
            SemTy::Refine {
                binder,
                predicate,
                base,
                name,
            } => {
                let base_value = self.assign(base, value, given, false, bounds)?;
                let pred = subst_var(predicate, binder, "it");
                let mut env = Env {
                    assumptions: self.env.assumptions.clone(),
                    ..Env::default()
                };
                if let Some(int) = &base_value.int {
                    env.ints.insert("it".to_string(), int.clone());
                }
                let mut local_bounds = bounds.clone();
                for fact in base_value.facts.iter().chain(value.facts.iter()) {
                    tighten(fact, &env, &mut local_bounds);
                }
                let mut assumptions = match decide(&mut self.steps, &env, &pred, &local_bounds) {
                    Pred::True { assumptions } => assumptions,
                    Pred::False { .. } => {
                        return Err(Disposition::Violated {
                            predicate: render_expr(predicate),
                            at: render_int(base_value.int.as_ref()),
                        });
                    }
                    Pred::Unknown(reason) => return Err(Disposition::Unknown { reason }),
                };
                assumptions = union_assumptions(
                    assumptions,
                    base_value
                        .assumptions
                        .iter()
                        .cloned()
                        .chain(value.assumptions.iter().cloned()),
                );
                if !assumptions.iter().all(|name| given.contains(name)) {
                    return Err(Disposition::Unknown {
                        reason: Reason::AssumptionRequired {
                            assumptions: assumptions.clone(),
                        },
                    });
                }
                let receipt = issue(
                    format!(
                        "{}({})",
                        name.clone().unwrap_or_else(|| target.render()),
                        render_int(base_value.int.as_ref())
                    ),
                    render_int(base_value.int.as_ref()),
                    PROFILE.to_string(),
                    if base_value.int.as_ref().is_some_and(IntExpr::is_concrete) {
                        Authority::RuntimeCheck
                    } else {
                        Authority::BoundedFragment
                    },
                    assumptions.clone(),
                    None,
                );
                if demand_kernel || receipt.authority == Authority::ProofKernel {
                    return Err(Disposition::Unknown {
                        reason: Reason::InsufficientAuthority {
                            required: "ProofKernel".to_string(),
                            found: if demand_kernel {
                                found_authority
                            } else {
                                "RuntimeCheck".to_string()
                            },
                        },
                    });
                }
                Ok(Value {
                    ty: target.clone(),
                    int: base_value.int.clone(),
                    bool: None,
                    facts: vec![pred],
                    assumptions,
                    receipt: Some(receipt),
                    conversion: None,
                })
            }
        }
    }

    fn require_ensures(
        &mut self,
        name: &str,
        ensures: &Expr,
        body: &Value,
        env: &Env,
        bounds: &BTreeMap<String, (i64, i64)>,
    ) -> Result<Vec<String>, Disposition> {
        let mut ensured = env.clone();
        if let Some(int) = &body.int {
            ensured.ints.insert("result".to_string(), int.clone());
        }
        let mut ensured_bounds = bounds.clone();
        for fact in &body.facts {
            let rewritten = subst_var(fact, "it", "result");
            tighten(&rewritten, &ensured, &mut ensured_bounds);
        }
        ensured.assumptions = union_names(ensured.assumptions, body.assumptions.iter().cloned());
        match decide(&mut self.steps, &ensured, ensures, &ensured_bounds) {
            Pred::True { assumptions } => {
                Ok(union_assumptions(assumptions, body.assumptions.clone()))
            }
            Pred::False { .. } => Err(Disposition::Violated {
                predicate: render_expr(ensures),
                at: name.to_string(),
            }),
            Pred::Unknown(reason) => Err(Disposition::Unknown { reason }),
        }
    }

    fn eval_expr(
        &mut self,
        expr: &Expr,
        env: &Env,
        bounds: &BTreeMap<String, (i64, i64)>,
        path: &str,
    ) -> Result<Value, Disposition> {
        self.charge()?;
        match expr {
            Expr::Num(text) => {
                let n = parse_i64(text)?;
                Ok(int_value(IntExpr::Concrete(n)))
            }
            Expr::Bool(v) => Ok(Value {
                ty: SemTy::Bool,
                int: None,
                bool: Some(*v),
                facts: Vec::new(),
                assumptions: Vec::new(),
                receipt: None,
                conversion: None,
            }),
            Expr::Var(name) => {
                if let Some(value) = env.values.get(name) {
                    return Ok(value.clone());
                }
                if let Some(value) = self.env.values.get(name) {
                    return Ok(value.clone());
                }
                if env.assumptions.contains(name) || self.env.assumptions.contains(name) {
                    return Ok(Value {
                        ty: SemTy::Bool,
                        int: None,
                        bool: None,
                        facts: Vec::new(),
                        assumptions: vec![name.clone()],
                        receipt: None,
                        conversion: None,
                    });
                }
                Err(Disposition::Unknown {
                    reason: Reason::MissingEvidence,
                })
            }
            Expr::Bin { op, lhs, rhs } => self.eval_bin(*op, lhs, rhs, env, bounds, path),
            Expr::Not(inner) => {
                let value = self.eval_expr(inner, env, bounds, path)?;
                let flag = value.bool.ok_or_else(|| Disposition::Unknown {
                    reason: Reason::UnsupportedFragment {
                        what: "negation of a non-boolean".to_string(),
                    },
                })?;
                Ok(Value {
                    ty: SemTy::Bool,
                    int: None,
                    bool: Some(!flag),
                    facts: Vec::new(),
                    assumptions: value.assumptions,
                    receipt: None,
                    conversion: None,
                })
            }
            Expr::Call { func, args } if func == "convert" => {
                if args.len() != 2 {
                    return Err(Disposition::Unknown {
                        reason: Reason::Malformed {
                            what: "convert takes a rule and a value".to_string(),
                        },
                    });
                }
                let Expr::Var(rule) = &args[0] else {
                    return Err(Disposition::Unknown {
                        reason: Reason::UnsupportedFragment {
                            what: "conversion rule must be a name".to_string(),
                        },
                    });
                };
                if rule != "identity" {
                    return Err(Disposition::Unknown {
                        reason: Reason::UnsupportedFragment {
                            what: format!("conversion rule {rule}"),
                        },
                    });
                }
                let value = self.eval_expr(&args[1], env, bounds, path)?;
                if value.int.is_none() {
                    return Err(Disposition::Unknown {
                        reason: Reason::UnsupportedFragment {
                            what: "identity conversion requires an Int representation".to_string(),
                        },
                    });
                }
                Ok(Value {
                    ty: SemTy::Int,
                    int: value.int.clone(),
                    bool: None,
                    facts: Vec::new(),
                    assumptions: value.assumptions.clone(),
                    receipt: None,
                    conversion: Some(Conversion {
                        rule: rule.clone(),
                        from: value.ty.render(),
                    }),
                })
            }
            Expr::Call { func, args } => self.eval_call(func, args, env, bounds, path),
            Expr::Validate { ty, arg } => {
                let sem = self.resolve_ty(ty, &BTreeMap::new())?;
                if !matches!(sem, SemTy::Refine { .. }) {
                    return Err(Disposition::Unknown {
                        reason: Reason::UnsupportedFragment {
                            what: "validate requires a refinement".to_string(),
                        },
                    });
                }
                let arg = self.eval_expr(arg, env, bounds, path)?;
                Ok(Value {
                    ty: SemTy::Validation {
                        refined: Box::new(sem),
                    },
                    int: arg.int.clone(),
                    bool: None,
                    facts: Vec::new(),
                    assumptions: arg.assumptions.clone(),
                    receipt: None,
                    conversion: None,
                })
            }
            Expr::Match {
                scrutinee,
                arms,
                proving_exhaustive,
            } => {
                if *proving_exhaustive {
                    return Err(Disposition::Unknown {
                        reason: Reason::InsufficientAuthority {
                            required: "a separate coverage certificate".to_string(),
                            found: "proving exhaustive on an evidence match".to_string(),
                        },
                    });
                }
                self.eval_match(scrutinee, arms, env, bounds, path)
            }
            other => Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: format!("expression {other:?}"),
                },
            }),
        }
    }

    fn eval_bin(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        env: &Env,
        bounds: &BTreeMap<String, (i64, i64)>,
        path: &str,
    ) -> Result<Value, Disposition> {
        if matches!(
            op,
            BinOp::Mul | BinOp::Div | BinOp::In | BinOp::Then | BinOp::And
        ) {
            return Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: format!("operator {op:?} is outside the evidence predicate fragment"),
                },
            });
        }
        let left = self.eval_expr(lhs, env, bounds, path)?;
        let right = self.eval_expr(rhs, env, bounds, path)?;
        if op.is_logical() {
            let l = left.bool;
            let r = right.bool;
            let flag = match (op, l, r) {
                (BinOp::AndAnd, Some(false), _) | (BinOp::OrOr, Some(true), _) => l,
                (BinOp::AndAnd, Some(true), Some(b)) | (BinOp::OrOr, Some(false), Some(b)) => {
                    Some(b)
                }
                _ => None,
            };
            let mut assumptions = left.assumptions;
            assumptions.extend(right.assumptions);
            return Ok(Value {
                ty: SemTy::Bool,
                int: None,
                bool: flag,
                facts: Vec::new(),
                assumptions,
                receipt: None,
                conversion: None,
            });
        }
        if op.is_comparison() {
            let pred = Expr::Bin {
                op,
                lhs: Box::new(lhs.clone()),
                rhs: Box::new(rhs.clone()),
            };
            let mut local = env.clone();
            local.ints.extend(self.env.ints.clone());
            match decide(&mut self.steps, &local, &pred, bounds) {
                Pred::True { assumptions } => {
                    return Ok(Value {
                        ty: SemTy::Bool,
                        int: None,
                        bool: Some(true),
                        facts: Vec::new(),
                        assumptions,
                        receipt: None,
                        conversion: None,
                    });
                }
                Pred::False { assumptions } => {
                    return Ok(Value {
                        ty: SemTy::Bool,
                        int: None,
                        bool: Some(false),
                        facts: Vec::new(),
                        assumptions,
                        receipt: None,
                        conversion: None,
                    });
                }
                Pred::Unknown(reason) => return Err(Disposition::Unknown { reason }),
            }
        }
        let l = left.int.ok_or_else(|| Disposition::Unknown {
            reason: Reason::UnsupportedFragment {
                what: "arithmetic on a non-integer".to_string(),
            },
        })?;
        let r = right.int.ok_or_else(|| Disposition::Unknown {
            reason: Reason::UnsupportedFragment {
                what: "arithmetic on a non-integer".to_string(),
            },
        })?;
        let int = match op {
            BinOp::Add => add_expr(&l, &r, bounds)?,
            BinOp::Sub => sub_expr(&l, &r, bounds)?,
            _ => {
                return Err(Disposition::Unknown {
                    reason: Reason::UnsupportedFragment {
                        what: format!("{op:?}"),
                    },
                });
            }
        };
        Ok(int_value(int))
    }

    fn eval_call(
        &mut self,
        func: &str,
        args: &[Expr],
        env: &Env,
        bounds: &BTreeMap<String, (i64, i64)>,
        path: &str,
    ) -> Result<Value, Disposition> {
        self.ensure_fun(func)?;
        let fun = self.funs.get(func).cloned().ok_or(Disposition::Unknown {
            reason: Reason::MissingEvidence,
        })?;
        if args.len() != fun.params.len() {
            return Err(Disposition::Unknown {
                reason: Reason::Malformed {
                    what: format!(
                        "{func} expects {} arguments, found {}",
                        fun.params.len(),
                        args.len()
                    ),
                },
            });
        }
        let mut evaluated = Vec::new();
        for arg in args {
            evaluated.push(self.eval_expr(arg, env, bounds, path)?);
        }
        let mut subst = BTreeMap::new();
        for ((param, formal), actual) in fun.params.iter().zip(evaluated.iter()) {
            unify_ty(formal, &actual.ty, &fun.type_params, param, &mut subst)?;
        }
        let mut call_env = Env {
            assumptions: self.env.assumptions.clone(),
            ..Env::default()
        };
        let mut call_bounds = BTreeMap::new();
        let mut call_assumptions = Vec::new();
        for ((param, formal), actual) in fun.params.iter().zip(evaluated.iter()) {
            let formal = instantiate_ty(formal, &subst);
            let actual = self.assign(&formal, actual, &[], false, bounds)?;
            if let Some(int) = &actual.int {
                call_env.ints.insert(param.clone(), int.clone());
            }
            for fact in &actual.facts {
                let rewritten = subst_var(fact, "it", param);
                tighten(&rewritten, &call_env, &mut call_bounds);
            }
            call_env.values.insert(param.clone(), actual.clone());
            call_env.assumptions = union_names(call_env.assumptions, actual.assumptions.clone());
            call_assumptions = union_assumptions(call_assumptions, actual.assumptions);
        }
        if let Some(requires) = &fun.requires {
            let requires = instantiate_expr(requires, &subst);
            match decide(&mut self.steps, &call_env, &requires, &call_bounds) {
                Pred::True { assumptions } => {
                    call_assumptions = union_assumptions(call_assumptions, assumptions);
                }
                Pred::False { .. } => {
                    return Err(Disposition::Violated {
                        predicate: render_expr(&requires),
                        at: func.to_string(),
                    });
                }
                Pred::Unknown(_) => {
                    return Err(Disposition::Unknown {
                        reason: Reason::IncompatibleComposition {
                            obligation: render_expr(&requires),
                        },
                    });
                }
            }
        }
        let mut body_env = call_env.clone();
        body_env.values.extend(self.env.values.clone());
        let body = self.eval_expr(&fun.body, &body_env, &call_bounds, path)?;
        let mut returned = body;
        returned.assumptions =
            union_assumptions(returned.assumptions, call_assumptions.iter().cloned());
        if let Some(ensures) = &fun.ensures {
            let ensures = instantiate_expr(ensures, &subst);
            let used = self.require_ensures(func, &ensures, &returned, &call_env, &call_bounds)?;
            call_assumptions = union_assumptions(call_assumptions, used);
            returned.assumptions =
                union_assumptions(returned.assumptions, call_assumptions.iter().cloned());
            returned.facts.push(subst_var(&ensures, "result", "it"));
        }
        if let Some(rule) = &fun.converts {
            returned.conversion = Some(Conversion {
                rule: rule.clone(),
                from: evaluated
                    .first()
                    .map(|value| value.ty.render())
                    .unwrap_or_else(|| "Int".to_string()),
            });
        }
        if let Some(ret) = &fun.ret {
            let ret = instantiate_ty(ret, &subst);
            returned = self.assign(&ret, &returned, &call_assumptions, false, &call_bounds)?;
        }
        returned.assumptions =
            union_assumptions(returned.assumptions, call_assumptions.iter().cloned());
        Ok(returned)
    }

    fn eval_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[ast::MatchArm],
        env: &Env,
        bounds: &BTreeMap<String, (i64, i64)>,
        path: &str,
    ) -> Result<Value, Disposition> {
        let is_validate = matches!(scrutinee, Expr::Validate { .. });
        let scrut = self.eval_expr(scrutinee, env, bounds, path)?;
        if is_validate {
            return self.eval_validate_match(&scrut, arms, env, bounds, path);
        }
        let mut reachable_values = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            if !arm_reachable(&arm.pattern, &scrut) {
                continue;
            }
            let mut arm_env = env.clone();
            bind_pattern(&arm.pattern, &scrut, &mut arm_env);
            let value = self.eval_expr(&arm.body, &arm_env, bounds, &format!("{path}/{index}"))?;
            reachable_values.push(value);
        }
        join_values(&reachable_values)
    }

    fn eval_validate_match(
        &mut self,
        scrut: &Value,
        arms: &[ast::MatchArm],
        env: &Env,
        bounds: &BTreeMap<String, (i64, i64)>,
        path: &str,
    ) -> Result<Value, Disposition> {
        let present = validate_arms(arms);
        if !present {
            return Err(Disposition::Unknown {
                reason: Reason::NonExhaustiveValidate,
            });
        }
        let refined = match &scrut.ty {
            SemTy::Validation { refined } => refined.as_ref().clone(),
            _ => {
                return Err(Disposition::Unknown {
                    reason: Reason::UnsupportedFragment {
                        what: "validate requires a refinement".to_string(),
                    },
                });
            }
        };
        let predicate_result = match &refined {
            SemTy::Refine {
                binder, predicate, ..
            } => {
                let pred = subst_var(predicate, binder, "it");
                let mut local = Env::default();
                if let Some(int) = &scrut.int {
                    local.ints.insert("it".to_string(), int.clone());
                }
                decide(&mut self.steps, &local, &pred, bounds)
            }
            _ => {
                return Err(Disposition::Unknown {
                    reason: Reason::UnsupportedFragment {
                        what: "validate requires a refinement".to_string(),
                    },
                });
            }
        };
        let concrete = scrut.int.as_ref().is_some_and(IntExpr::is_concrete);
        let taken = if concrete {
            Some(match &predicate_result {
                Pred::True { .. } => "Satisfied",
                Pred::False { .. } => "Violated",
                Pred::Unknown(_) => "Unknown",
            })
        } else {
            None
        };
        let satisfied_value = match &predicate_result {
            Pred::True { assumptions } => {
                let receipt = issue(
                    format!("{}({})", refined.render(), render_int(scrut.int.as_ref())),
                    render_int(scrut.int.as_ref()),
                    PROFILE.to_string(),
                    Authority::RuntimeCheck,
                    assumptions.clone(),
                    None,
                );
                let mut value = scrut.clone();
                value.ty = refined.clone();
                value.receipt = Some(receipt);
                value.assumptions = assumptions.clone();
                if let SemTy::Refine {
                    predicate, binder, ..
                } = &refined
                {
                    value.facts = vec![subst_var(predicate, binder, "it")];
                }
                value
            }
            _ => {
                let mut value = scrut.clone();
                value.ty = refined.clone();
                value.receipt = None;
                value.facts.clear();
                value
            }
        };
        let mut results = Vec::new();
        for arm in arms {
            let Some(kind) = ctor_name(&arm.pattern) else {
                return Err(Disposition::Unknown {
                    reason: Reason::NonExhaustiveValidate,
                });
            };
            let reachable = taken.is_none_or(|taken| taken == kind);
            let mut arm_env = env.clone();
            if kind == "Satisfied" {
                if let Pattern::Ctor { args, .. } = &arm.pattern {
                    if let Some(Pattern::Var(name)) = args.first() {
                        arm_env.values.insert(name.clone(), satisfied_value.clone());
                        if let Some(int) = &satisfied_value.int {
                            arm_env.ints.insert(name.clone(), int.clone());
                        }
                        self.report.arms.push(ArmBinding {
                            path: format!("{path}/Satisfied/{name}"),
                            ty: refined.render(),
                            reachable,
                        });
                    }
                    if args
                        .get(1)
                        .is_some_and(|pat| matches!(pat, Pattern::Var(_)))
                    {
                        // The receipt binding is evidence of the runtime check.
                        // It is not a kernel certificate.
                    }
                }
            }
            if reachable {
                let value = self.eval_expr(&arm.body, &arm_env, bounds, path)?;
                results.push(value);
            }
        }
        if let Pred::Unknown(Reason::BudgetExhausted) = &predicate_result {
            return Err(Disposition::Unknown {
                reason: Reason::BudgetExhausted,
            });
        }
        join_values(&results)
    }

    fn resolve_ty(
        &self,
        ty: &ast::Ty,
        indices: &BTreeMap<String, SemIndex>,
    ) -> Result<SemTy, Disposition> {
        match ty {
            ast::Ty::Graded(inner, _) | ast::Ty::Given { inner, .. } => {
                self.resolve_ty(inner, indices)
            }
            ast::Ty::Named(name) if name == "Int" => Ok(SemTy::Int),
            ast::Ty::Named(name) if name == "Bool" => Ok(SemTy::Bool),
            ast::Ty::Named(name) => {
                if let Some(index) = indices.get(name) {
                    return match index {
                        SemIndex::Tag(tag) => Ok(SemTy::Nominal {
                            name: tag.clone(),
                            indices: Vec::new(),
                        }),
                        SemIndex::Int(_) => Ok(SemTy::Int),
                    };
                }
                self.lookup_named(name, &[])
            }
            ast::Ty::App(name, args) => {
                let mut resolved = Vec::new();
                for arg in args {
                    resolved.push(self.resolve_index(arg, indices)?);
                }
                self.lookup_named(name, &resolved)
            }
            ast::Ty::Refine {
                binder,
                base,
                predicate,
            } => {
                let base = self.resolve_ty(base, indices)?;
                if base != SemTy::Int {
                    return Err(Disposition::Unknown {
                        reason: Reason::UnsupportedFragment {
                            what: "refinement base other than Int".to_string(),
                        },
                    });
                }
                Ok(SemTy::Refine {
                    name: None,
                    binder: binder.clone(),
                    predicate: subst_indices(predicate, indices),
                    base: Box::new(base),
                })
            }
            ast::Ty::Scalar(_) => Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: "a scalar index is not a value type".to_string(),
                },
            }),
            ast::Ty::Record(_) => Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: "record types in the evidence profile".to_string(),
                },
            }),
        }
    }

    fn resolve_index(
        &self,
        ty: &ast::Ty,
        indices: &BTreeMap<String, SemIndex>,
    ) -> Result<SemIndex, Disposition> {
        match ty {
            ast::Ty::Scalar(n) => Ok(SemIndex::Int(*n)),
            ast::Ty::Named(name) => {
                if let Some(index) = indices.get(name) {
                    Ok(index.clone())
                } else {
                    Ok(SemIndex::Tag(name.clone()))
                }
            }
            _ => Err(Disposition::Unknown {
                reason: Reason::UnsupportedFragment {
                    what: "type argument in an evidence index".to_string(),
                },
            }),
        }
    }

    fn lookup_named(&self, name: &str, args: &[SemIndex]) -> Result<SemTy, Disposition> {
        match self.decls.get(name) {
            Some(Declared::Newtype { params }) => {
                if params.len() != args.len() {
                    return Err(Disposition::Unknown {
                        reason: Reason::Malformed {
                            what: format!(
                                "{name} expects {} indices, found {}",
                                params.len(),
                                args.len()
                            ),
                        },
                    });
                }
                Ok(SemTy::Nominal {
                    name: name.to_string(),
                    indices: args.to_vec(),
                })
            }
            Some(Declared::Alias { params, body }) => {
                if params.len() != args.len() {
                    return Err(Disposition::Unknown {
                        reason: Reason::Malformed {
                            what: format!(
                                "{name} expects {} indices, found {}",
                                params.len(),
                                args.len()
                            ),
                        },
                    });
                }
                let mut subst = BTreeMap::new();
                for ((param, is_int), arg) in params.iter().zip(args) {
                    if *is_int && !matches!(arg, SemIndex::Int(_)) {
                        return Err(Disposition::Unknown {
                            reason: Reason::Malformed {
                                what: format!("{param} requires an integer index"),
                            },
                        });
                    }
                    subst.insert(param.clone(), arg.clone());
                }
                Ok(instantiate_sem(body, &subst, name))
            }
            None => Err(Disposition::Unknown {
                reason: Reason::MissingEvidence,
            }),
        }
    }
}

fn fun_placeholder(fun: &ast::Callable) -> FunSig {
    FunSig {
        type_params: fun.type_params.clone(),
        params: Vec::new(),
        ret: None,
        requires: fun.requires.clone(),
        ensures: fun.ensures.clone(),
        converts: fun.converts.clone(),
        body: fun.body.clone(),
    }
}

fn index_env(params: &[ast::IndexParam]) -> BTreeMap<String, SemIndex> {
    params
        .iter()
        .map(|param| (param.name.clone(), SemIndex::Tag(param.name.clone())))
        .collect()
}

fn ty_binder(ty: &SemTy) -> &str {
    match ty {
        SemTy::Refine { binder, .. } => binder,
        _ => "it",
    }
}

fn symbolic_param(name: &str, ty: &SemTy) -> Value {
    let facts = match ty {
        SemTy::Refine {
            predicate, binder, ..
        } => {
            vec![subst_var(predicate, binder, "it")]
        }
        _ => Vec::new(),
    };
    Value {
        ty: ty.clone(),
        int: Some(IntExpr::Sym(name.to_string())),
        bool: None,
        facts,
        assumptions: Vec::new(),
        receipt: None,
        conversion: None,
    }
}

fn int_value(int: IntExpr) -> Value {
    Value {
        ty: SemTy::Int,
        int: Some(int),
        bool: None,
        facts: Vec::new(),
        assumptions: Vec::new(),
        receipt: None,
        conversion: None,
    }
}

fn instantiate_sem(ty: &SemTy, subst: &BTreeMap<String, SemIndex>, alias: &str) -> SemTy {
    match ty {
        SemTy::Refine {
            binder,
            predicate,
            base,
            ..
        } => SemTy::Refine {
            name: Some(alias.to_string()),
            binder: binder.clone(),
            predicate: subst_indices(predicate, subst),
            base: base.clone(),
        },
        other => other.clone(),
    }
}

fn instantiate_ty(ty: &SemTy, subst: &BTreeMap<String, SemIndex>) -> SemTy {
    match ty {
        SemTy::Nominal { name, indices } => {
            if let Some(SemIndex::Tag(tag)) = subst.get(name) {
                if indices.is_empty() {
                    return SemTy::Nominal {
                        name: tag.clone(),
                        indices: Vec::new(),
                    };
                }
            }
            SemTy::Nominal {
                name: name.clone(),
                indices: indices
                    .iter()
                    .map(|index| match index {
                        SemIndex::Tag(tag) => {
                            subst.get(tag).cloned().unwrap_or_else(|| index.clone())
                        }
                        SemIndex::Int(_) => index.clone(),
                    })
                    .collect(),
            }
        }
        SemTy::Refine {
            name,
            binder,
            predicate,
            base,
        } => SemTy::Refine {
            name: name.clone(),
            binder: binder.clone(),
            predicate: subst_indices(predicate, subst),
            base: Box::new(instantiate_ty(base, subst)),
        },
        other => other.clone(),
    }
}

fn unify_ty(
    formal: &SemTy,
    actual: &SemTy,
    type_params: &[String],
    param_name: &str,
    subst: &mut BTreeMap<String, SemIndex>,
) -> Result<(), Disposition> {
    match (formal, actual) {
        (
            SemTy::Nominal {
                name,
                indices: formal_idx,
            },
            SemTy::Nominal {
                name: actual_name,
                indices: actual_idx,
            },
        ) if name == actual_name && formal_idx.len() == actual_idx.len() => {
            for (formal_index, actual_index) in formal_idx.iter().zip(actual_idx) {
                if let SemIndex::Tag(tag) = formal_index {
                    if type_params.iter().any(|param| param == tag) {
                        if let Some(previous) = subst.get(tag) {
                            if previous != actual_index {
                                return Err(Disposition::Unknown {
                                    reason: Reason::MissingObligation {
                                        obligation: format!(
                                            "type parameter {tag} is {previous:?} and {actual_index:?} at {param_name}"
                                        ),
                                    },
                                });
                            }
                        } else {
                            subst.insert(tag.clone(), actual_index.clone());
                        }
                        continue;
                    }
                }
                if formal_index != actual_index {
                    return Err(Disposition::Unknown {
                        reason: Reason::NominalMismatch {
                            expected: format!("{formal_index:?}"),
                            found: format!("{actual_index:?}"),
                        },
                    });
                }
            }
            Ok(())
        }
        (SemTy::Int, SemTy::Int) | (SemTy::Bool, SemTy::Bool) => Ok(()),
        (SemTy::Refine { base, .. }, _) => unify_ty(base, actual, type_params, param_name, subst),
        _ if formal.render() == actual.render() => Ok(()),
        _ => Err(Disposition::Unknown {
            reason: Reason::NominalMismatch {
                expected: formal.render(),
                found: actual.render(),
            },
        }),
    }
}

fn join_values(values: &[Value]) -> Result<Value, Disposition> {
    let Some(first) = values.first() else {
        return Err(Disposition::Unknown {
            reason: Reason::MissingEvidence,
        });
    };
    let mut ty = first.ty.clone();
    for value in values.iter().skip(1) {
        ty = match (&ty, &value.ty) {
            (a, b) if a == b => a.clone(),
            (SemTy::Refine { base, .. }, other) | (other, SemTy::Refine { base, .. })
                if base.as_ref() == other
                    || matches!(other, SemTy::Refine { base: b, .. } if b == base) =>
            {
                SemTy::Int
            }
            (a, b) if a.render() == b.render() => a.clone(),
            _ => {
                return Err(Disposition::Unknown {
                    reason: Reason::UnresolvedJoin {
                        claim: format!("{} and {}", ty.render(), value.ty.render()),
                    },
                });
            }
        };
    }
    let assumptions = values.iter().fold(Vec::new(), |acc, value| {
        union_assumptions(acc, value.assumptions.iter().cloned())
    });
    let widened = values.iter().any(|value| value.ty != ty);
    if values_agree(values) && !widened {
        let mut joined = first.clone();
        joined.ty = ty;
        joined.assumptions = assumptions.clone();
        if let Some(receipt) = joined.receipt {
            joined.receipt = Some(reissue_receipt(receipt, assumptions));
        }
        return Ok(joined);
    }
    if values_agree(values) {
        return Ok(Value {
            ty,
            int: first.int.clone(),
            bool: first.bool,
            facts: Vec::new(),
            assumptions,
            receipt: None,
            conversion: None,
        });
    }
    // The branches do not establish one integer or one certificate. Keep the
    // joined type and the assumptions any branch required, and leave the
    // value unknown so a later obligation cannot be discharged by one arm.
    Ok(Value {
        ty,
        int: None,
        bool: None,
        facts: Vec::new(),
        assumptions,
        receipt: None,
        conversion: None,
    })
}

/// Equal types are not equal evidence. A join keeps one value only when every
/// reachable branch has the same integer, boolean, facts, and certificate.
fn values_agree(values: &[Value]) -> bool {
    let Some(first) = values.first() else {
        return true;
    };
    values.iter().skip(1).all(|value| {
        value.int == first.int
            && value.bool == first.bool
            && value.facts == first.facts
            && value.conversion == first.conversion
            && receipts_agree(&value.receipt, &first.receipt)
    })
}

fn receipts_agree(left: &Option<Receipt>, right: &Option<Receipt>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.proposition == right.proposition
                && left.subject == right.subject
                && left.context == right.context
                && left.authority == right.authority
                && left.kernel_certificate == right.kernel_certificate
        }
        _ => false,
    }
}

fn reissue_receipt(receipt: Receipt, assumptions: Vec<String>) -> Receipt {
    if receipt.assumptions == assumptions {
        return receipt;
    }
    issue(
        receipt.proposition,
        receipt.subject,
        receipt.context,
        receipt.authority,
        assumptions,
        receipt.kernel_certificate,
    )
}

fn arm_reachable(pattern: &Pattern, value: &Value) -> bool {
    match pattern {
        Pattern::Wildcard | Pattern::Var(_) => true,
        Pattern::Ctor { name, .. } => match (name.as_str(), value.bool) {
            ("true", Some(true)) | ("false", Some(false)) => true,
            ("true" | "false", None) => true,
            ("true", Some(false)) | ("false", Some(true)) => false,
            _ => true,
        },
    }
}

fn bind_pattern(pattern: &Pattern, value: &Value, env: &mut Env) {
    match pattern {
        Pattern::Var(name) => {
            env.values.insert(name.clone(), value.clone());
            if let Some(int) = &value.int {
                env.ints.insert(name.clone(), int.clone());
            }
        }
        Pattern::Ctor { args, .. } => {
            for arg in args {
                bind_pattern(arg, value, env);
            }
        }
        Pattern::Wildcard => {}
    }
}

fn validate_arms(arms: &[ast::MatchArm]) -> bool {
    let mut seen = BTreeSet::new();
    for arm in arms {
        match &arm.pattern {
            Pattern::Ctor { name, .. }
                if matches!(name.as_str(), "Satisfied" | "Violated" | "Unknown") =>
            {
                seen.insert(name.clone());
            }
            _ => return false,
        }
    }
    seen.len() == 3
}

fn ctor_name(pattern: &Pattern) -> Option<&str> {
    match pattern {
        Pattern::Ctor { name, .. } => Some(name),
        _ => None,
    }
}

fn add_expr(
    left: &IntExpr,
    right: &IntExpr,
    bounds: &BTreeMap<String, (i64, i64)>,
) -> Result<IntExpr, Disposition> {
    if let (Some(a), Some(b)) = (left.concrete(), right.concrete()) {
        return a
            .checked_add(b)
            .map(IntExpr::Concrete)
            .ok_or_else(|| Disposition::Unknown {
                reason: Reason::Definedness {
                    op: "addition".to_string(),
                },
            });
    }
    if !op_defined(left, right, bounds, true) {
        return Err(Disposition::Unknown {
            reason: Reason::Definedness {
                op: "addition".to_string(),
            },
        });
    }
    Ok(IntExpr::Add(
        Box::new(left.clone()),
        Box::new(right.clone()),
    ))
}

fn sub_expr(
    left: &IntExpr,
    right: &IntExpr,
    bounds: &BTreeMap<String, (i64, i64)>,
) -> Result<IntExpr, Disposition> {
    if let (Some(a), Some(b)) = (left.concrete(), right.concrete()) {
        return a
            .checked_sub(b)
            .map(IntExpr::Concrete)
            .ok_or_else(|| Disposition::Unknown {
                reason: Reason::Definedness {
                    op: "subtraction".to_string(),
                },
            });
    }
    if !op_defined(left, right, bounds, false) {
        return Err(Disposition::Unknown {
            reason: Reason::Definedness {
                op: "subtraction".to_string(),
            },
        });
    }
    Ok(IntExpr::Sub(
        Box::new(left.clone()),
        Box::new(right.clone()),
    ))
}

fn op_defined(
    left: &IntExpr,
    right: &IntExpr,
    bounds: &BTreeMap<String, (i64, i64)>,
    add: bool,
) -> bool {
    let Some((l0, l1)) = interval_of(left, bounds) else {
        return false;
    };
    let Some((r0, r1)) = interval_of(right, bounds) else {
        return false;
    };
    if add {
        l0.checked_add(r0).is_some() && l1.checked_add(r1).is_some()
    } else {
        l0.checked_sub(r1).is_some() && l1.checked_sub(r0).is_some()
    }
}

fn interval_of(expr: &IntExpr, bounds: &BTreeMap<String, (i64, i64)>) -> Option<(i64, i64)> {
    match expr {
        IntExpr::Concrete(n) => Some((*n, *n)),
        IntExpr::Sym(name) => bounds.get(name).copied().or(Some((i64::MIN, i64::MAX))),
        IntExpr::Add(a, b) => {
            let (a0, a1) = interval_of(a, bounds)?;
            let (b0, b1) = interval_of(b, bounds)?;
            Some((a0.checked_add(b0)?, a1.checked_add(b1)?))
        }
        IntExpr::Sub(a, b) => {
            let (a0, a1) = interval_of(a, bounds)?;
            let (b0, b1) = interval_of(b, bounds)?;
            Some((a0.checked_sub(b1)?, a1.checked_sub(b0)?))
        }
    }
}

impl IntExpr {
    fn concrete(&self) -> Option<i64> {
        match self {
            IntExpr::Concrete(n) => Some(*n),
            _ => None,
        }
    }

    fn is_concrete(&self) -> bool {
        self.concrete().is_some()
    }
}

fn decide(steps: &mut u32, env: &Env, expr: &Expr, bounds: &BTreeMap<String, (i64, i64)>) -> Pred {
    if *steps == 0 {
        return Pred::Unknown(Reason::BudgetExhausted);
    }
    *steps -= 1;
    match expr {
        Expr::Bool(true) => Pred::True {
            assumptions: Vec::new(),
        },
        Expr::Bool(false) => Pred::False {
            assumptions: Vec::new(),
        },
        Expr::Var(name) => {
            if env.assumptions.contains(name) {
                Pred::True {
                    assumptions: vec![name.clone()],
                }
            } else if let Some(flag) = env.bools.get(name) {
                if *flag {
                    Pred::True {
                        assumptions: Vec::new(),
                    }
                } else {
                    Pred::False {
                        assumptions: Vec::new(),
                    }
                }
            } else {
                Pred::Unknown(Reason::MissingEvidence)
            }
        }
        Expr::Not(inner) => match decide(steps, env, inner, bounds) {
            Pred::True { assumptions } => Pred::False { assumptions },
            Pred::False { assumptions } => Pred::True { assumptions },
            other => other,
        },
        Expr::Bin {
            op: BinOp::AndAnd,
            lhs,
            rhs,
        } => match decide(steps, env, lhs, bounds) {
            Pred::False { assumptions } => Pred::False { assumptions },
            Pred::True { assumptions } => match decide(steps, env, rhs, bounds) {
                Pred::True { assumptions: more } => Pred::True {
                    assumptions: union_assumptions(assumptions, more),
                },
                Pred::False { assumptions: more } if more.is_empty() => Pred::False {
                    assumptions: Vec::new(),
                },
                Pred::False { assumptions: more } => Pred::False {
                    assumptions: union_assumptions(assumptions, more),
                },
                other => other,
            },
            other => other,
        },
        Expr::Bin {
            op: BinOp::OrOr,
            lhs,
            rhs,
        } => match decide(steps, env, lhs, bounds) {
            Pred::True { assumptions } => Pred::True { assumptions },
            Pred::False { assumptions } if assumptions.is_empty() => {
                decide(steps, env, rhs, bounds)
            }
            Pred::False { assumptions } => match decide(steps, env, rhs, bounds) {
                Pred::True { assumptions: more } => Pred::True { assumptions: more },
                Pred::False { assumptions: more } => Pred::False {
                    assumptions: union_assumptions(assumptions, more),
                },
                other => other,
            },
            other => other,
        },
        Expr::Bin { op, lhs, rhs } if op.is_comparison() => {
            compare_pred(steps, env, *op, lhs, rhs, bounds)
        }
        _ => Pred::Unknown(Reason::UnsupportedFragment {
            what: format!("predicate {}", render_expr(expr)),
        }),
    }
}

fn compare_pred(
    steps: &mut u32,
    env: &Env,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    bounds: &BTreeMap<String, (i64, i64)>,
) -> Pred {
    let left = match int_of(steps, env, lhs, bounds) {
        Ok(int) => int,
        Err(reason) => return Pred::Unknown(reason),
    };
    let right = match int_of(steps, env, rhs, bounds) {
        Ok(int) => int,
        Err(reason) => return Pred::Unknown(reason),
    };
    if let (Some(a), Some(b)) = (left.concrete(), right.concrete()) {
        return if compare_i64(op, a, b) {
            Pred::True {
                assumptions: Vec::new(),
            }
        } else {
            Pred::False {
                assumptions: Vec::new(),
            }
        };
    }
    match algebraic_cmp(&left, &right) {
        Algebraic::Undefined => {
            return Pred::Unknown(Reason::Definedness {
                op: "comparison".to_string(),
            });
        }
        Algebraic::Equal => {
            return if compare_i64(op, 0, 0) {
                Pred::True {
                    assumptions: Vec::new(),
                }
            } else {
                Pred::False {
                    assumptions: Vec::new(),
                }
            };
        }
        Algebraic::ConstantDiff {
            left: lk,
            right: rk,
        } => {
            return if compare_i64(op, lk, rk) {
                Pred::True {
                    assumptions: Vec::new(),
                }
            } else {
                Pred::False {
                    assumptions: Vec::new(),
                }
            };
        }
        Algebraic::Unresolved => {}
    }
    let Some((l0, l1)) = interval_of(&left, bounds) else {
        return Pred::Unknown(Reason::Definedness {
            op: "comparison".to_string(),
        });
    };
    let Some((r0, r1)) = interval_of(&right, bounds) else {
        return Pred::Unknown(Reason::Definedness {
            op: "comparison".to_string(),
        });
    };
    let definitely = match op {
        BinOp::Lt => l1 < r0,
        BinOp::Le => l1 <= r0,
        BinOp::Gt => l0 > r1,
        BinOp::Ge => l0 >= r1,
        BinOp::Eq => l0 == l1 && r0 == r1 && l0 == r0,
        BinOp::Ne => l1 < r0 || r1 < l0,
        _ => false,
    };
    let impossible = match op {
        BinOp::Lt => l0 >= r1,
        BinOp::Le => l0 > r1,
        BinOp::Gt => l1 <= r0,
        BinOp::Ge => l1 < r0,
        BinOp::Eq => l1 < r0 || r1 < l0,
        BinOp::Ne => l0 == l1 && r0 == r1 && l0 == r0,
        _ => false,
    };
    if definitely {
        Pred::True {
            assumptions: Vec::new(),
        }
    } else if impossible {
        Pred::False {
            assumptions: Vec::new(),
        }
    } else {
        Pred::Unknown(Reason::MissingEvidence)
    }
}

fn compare_i64(op: BinOp, left: i64, right: i64) -> bool {
    match op {
        BinOp::Lt => left < right,
        BinOp::Le => left <= right,
        BinOp::Gt => left > right,
        BinOp::Ge => left >= right,
        BinOp::Eq => left == right,
        BinOp::Ne => left != right,
        _ => false,
    }
}

fn int_of(
    steps: &mut u32,
    env: &Env,
    expr: &Expr,
    bounds: &BTreeMap<String, (i64, i64)>,
) -> Result<IntExpr, Reason> {
    if *steps == 0 {
        return Err(Reason::BudgetExhausted);
    }
    *steps -= 1;
    match expr {
        Expr::Num(text) => {
            text.parse::<i64>()
                .map(IntExpr::Concrete)
                .map_err(|_| Reason::Malformed {
                    what: format!("integer {text}"),
                })
        }
        Expr::Var(name) => env.ints.get(name).cloned().ok_or(Reason::MissingEvidence),
        Expr::Bin {
            op: BinOp::Add,
            lhs,
            rhs,
        } => {
            let left = int_of(steps, env, lhs, bounds)?;
            let right = int_of(steps, env, rhs, bounds)?;
            if let (Some(a), Some(b)) = (left.concrete(), right.concrete()) {
                return a
                    .checked_add(b)
                    .map(IntExpr::Concrete)
                    .ok_or(Reason::Definedness {
                        op: "addition".to_string(),
                    });
            }
            if !op_defined(&left, &right, bounds, true) {
                return Err(Reason::Definedness {
                    op: "addition".to_string(),
                });
            }
            Ok(IntExpr::Add(Box::new(left), Box::new(right)))
        }
        Expr::Bin {
            op: BinOp::Sub,
            lhs,
            rhs,
        } => {
            let left = int_of(steps, env, lhs, bounds)?;
            let right = int_of(steps, env, rhs, bounds)?;
            if let (Some(a), Some(b)) = (left.concrete(), right.concrete()) {
                return a
                    .checked_sub(b)
                    .map(IntExpr::Concrete)
                    .ok_or(Reason::Definedness {
                        op: "subtraction".to_string(),
                    });
            }
            if !op_defined(&left, &right, bounds, false) {
                return Err(Reason::Definedness {
                    op: "subtraction".to_string(),
                });
            }
            Ok(IntExpr::Sub(Box::new(left), Box::new(right)))
        }
        _ => Err(Reason::UnsupportedFragment {
            what: format!("integer expression {}", render_expr(expr)),
        }),
    }
}

enum Algebraic {
    Equal,
    /// Same coefficients, so the values differ exactly by these constants.
    ConstantDiff {
        left: i64,
        right: i64,
    },
    /// Different coefficients. Equality is not settled by the linear form.
    Unresolved,
    Undefined,
}

/// Identical normalized forms are equal. Identical coefficients with different
/// constants are unequal. Different coefficients are unresolved: `x` and `y`
/// can still be the same value. Zero coefficients are dropped so `x - x` is `0`
/// when that subtraction is defined. Overflow while normalizing is undefined.
fn algebraic_cmp(left: &IntExpr, right: &IntExpr) -> Algebraic {
    let Ok((mut left_coeff, left_const)) = normal(left) else {
        return Algebraic::Undefined;
    };
    let Ok((mut right_coeff, right_const)) = normal(right) else {
        return Algebraic::Undefined;
    };
    left_coeff.retain(|_, coeff| *coeff != 0);
    right_coeff.retain(|_, coeff| *coeff != 0);
    if left_coeff == right_coeff && left_const == right_const {
        Algebraic::Equal
    } else if left_coeff == right_coeff {
        Algebraic::ConstantDiff {
            left: left_const,
            right: right_const,
        }
    } else {
        Algebraic::Unresolved
    }
}

fn union_assumptions(mut base: Vec<String>, more: impl IntoIterator<Item = String>) -> Vec<String> {
    for name in more {
        if !base.contains(&name) {
            base.push(name);
        }
    }
    base
}

fn union_names(
    mut base: BTreeSet<String>,
    more: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    base.extend(more);
    base
}

fn normal(expr: &IntExpr) -> Result<(BTreeMap<String, i64>, i64), ()> {
    match expr {
        IntExpr::Concrete(n) => Ok((BTreeMap::new(), *n)),
        IntExpr::Sym(name) => {
            let mut coeffs = BTreeMap::new();
            coeffs.insert(name.clone(), 1);
            Ok((coeffs, 0))
        }
        IntExpr::Add(a, b) => {
            let (mut coeffs, k) = normal(a)?;
            let (more, k2) = normal(b)?;
            for (name, coeff) in more {
                let slot = coeffs.entry(name).or_insert(0);
                *slot = slot.checked_add(coeff).ok_or(())?;
            }
            Ok((coeffs, k.checked_add(k2).ok_or(())?))
        }
        IntExpr::Sub(a, b) => {
            let (mut coeffs, k) = normal(a)?;
            let (more, k2) = normal(b)?;
            for (name, coeff) in more {
                let slot = coeffs.entry(name).or_insert(0);
                *slot = slot.checked_sub(coeff).ok_or(())?;
            }
            Ok((coeffs, k.checked_sub(k2).ok_or(())?))
        }
    }
}

fn tighten(expr: &Expr, env: &Env, bounds: &mut BTreeMap<String, (i64, i64)>) {
    match expr {
        Expr::Bin {
            op: BinOp::AndAnd,
            lhs,
            rhs,
        } => {
            tighten(lhs, env, bounds);
            tighten(rhs, env, bounds);
        }
        Expr::Bin { op, lhs, rhs } if op.is_comparison() => {
            if let (Expr::Var(name), Expr::Num(text)) = (lhs.as_ref(), rhs.as_ref()) {
                if let Ok(n) = text.parse::<i64>() {
                    bound_var(bounds, name, *op, n, true);
                }
            } else if let (Expr::Num(text), Expr::Var(name)) = (lhs.as_ref(), rhs.as_ref()) {
                if let Ok(n) = text.parse::<i64>() {
                    bound_var(bounds, name, *op, n, false);
                }
            } else if let (Expr::Var(name), Expr::Var(_)) = (lhs.as_ref(), rhs.as_ref()) {
                let _ = (name, env);
            }
        }
        _ => {}
    }
}

fn bound_var(
    bounds: &mut BTreeMap<String, (i64, i64)>,
    name: &str,
    op: BinOp,
    n: i64,
    var_on_left: bool,
) {
    let (mut lo, mut hi) = bounds.get(name).copied().unwrap_or((i64::MIN, i64::MAX));
    let op = if var_on_left { op } else { flip(op) };
    match op {
        BinOp::Lt => hi = hi.min(n.saturating_sub(1)),
        BinOp::Le => hi = hi.min(n),
        BinOp::Gt => lo = lo.max(n.saturating_add(1)),
        BinOp::Ge => lo = lo.max(n),
        BinOp::Eq => {
            lo = n;
            hi = n;
        }
        _ => {}
    }
    if lo <= hi {
        bounds.insert(name.to_string(), (lo, hi));
    }
}

fn flip(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        other => other,
    }
}

fn subst_var(expr: &Expr, from: &str, to: &str) -> Expr {
    match expr {
        Expr::Var(name) if name == from => Expr::Var(to.to_string()),
        Expr::Bin { op, lhs, rhs } => Expr::Bin {
            op: *op,
            lhs: Box::new(subst_var(lhs, from, to)),
            rhs: Box::new(subst_var(rhs, from, to)),
        },
        Expr::Not(inner) => Expr::Not(Box::new(subst_var(inner, from, to))),
        other => other.clone(),
    }
}

fn subst_indices(expr: &Expr, indices: &BTreeMap<String, SemIndex>) -> Expr {
    match expr {
        Expr::Var(name) => match indices.get(name) {
            Some(SemIndex::Int(n)) => Expr::Num(n.to_string()),
            Some(SemIndex::Tag(tag)) => Expr::Var(tag.clone()),
            None => expr.clone(),
        },
        Expr::Bin { op, lhs, rhs } => Expr::Bin {
            op: *op,
            lhs: Box::new(subst_indices(lhs, indices)),
            rhs: Box::new(subst_indices(rhs, indices)),
        },
        Expr::Not(inner) => Expr::Not(Box::new(subst_indices(inner, indices))),
        other => other.clone(),
    }
}

fn instantiate_expr(expr: &Expr, subst: &BTreeMap<String, SemIndex>) -> Expr {
    subst_indices(expr, subst)
}

fn split_given(ty: &ast::Ty) -> (&ast::Ty, Vec<String>) {
    match ty {
        ast::Ty::Given { inner, assumptions } => (inner, assumptions.clone()),
        ast::Ty::Graded(inner, _) => split_given(inner),
        other => (other, Vec::new()),
    }
}

fn demanded_kernel(ty: &ast::Ty) -> bool {
    match ty {
        ast::Ty::Graded(_, ast::Grade::Proven) => true,
        ast::Ty::Graded(inner, _) => demanded_kernel(inner),
        ast::Ty::Given { inner, .. } => demanded_kernel(inner),
        _ => false,
    }
}

fn parse_i64(text: &str) -> Result<i64, Disposition> {
    text.parse::<i64>().map_err(|_| Disposition::Unknown {
        reason: Reason::Malformed {
            what: format!("integer {text}"),
        },
    })
}

fn render_int(int: Option<&IntExpr>) -> String {
    match int {
        Some(IntExpr::Concrete(n)) => n.to_string(),
        Some(_) => "symbolic".to_string(),
        None => "unknown".to_string(),
    }
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Num(n) => n.clone(),
        Expr::Bool(v) => v.to_string(),
        Expr::Var(name) => name.clone(),
        Expr::Not(inner) => format!("!{}", render_expr(inner)),
        Expr::Bin { op, lhs, rhs } => {
            format!(
                "({} {} {})",
                render_expr(lhs),
                bin_op(*op),
                render_expr(rhs)
            )
        }
        _ => format!("{expr:?}"),
    }
}

fn bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::AndAnd => "&&",
        BinOp::OrOr => "||",
        _ => "?",
    }
}

fn item_opts_in(item: &Item) -> bool {
    match item {
        Item::Newtype(_) | Item::TypeDef(_) | Item::Assume(_) => true,
        Item::Fn(fun) | Item::Rule(fun) => {
            fun.requires.is_some()
                || fun.ensures.is_some()
                || fun.converts.is_some()
                || !fun.type_params.is_empty()
                || fun
                    .params
                    .iter()
                    .any(|param| param.ty.as_ref().is_some_and(ty_opts_in))
                || fun.ret.as_ref().is_some_and(ty_opts_in)
                || expr_opts_in(&fun.body)
        }
        Item::Let(binding) => {
            binding.ty.as_ref().is_some_and(ty_opts_in) || expr_opts_in(&binding.value)
        }
        _ => false,
    }
}

fn ty_opts_in(ty: &ast::Ty) -> bool {
    match ty {
        ast::Ty::Refine { .. } | ast::Ty::Scalar(_) | ast::Ty::Given { .. } => true,
        ast::Ty::Graded(inner, _) => ty_opts_in(inner),
        ast::Ty::App(_, args) => args.iter().any(ty_opts_in),
        ast::Ty::Record(fields) => fields.iter().any(|field| ty_opts_in(&field.ty)),
        ast::Ty::Named(_) => false,
    }
}

fn expr_opts_in(expr: &Expr) -> bool {
    match expr {
        Expr::Validate { .. } => true,
        Expr::Call { func, args } if func == "convert" => {
            let _ = args;
            true
        }
        Expr::Bin { lhs, rhs, .. } => expr_opts_in(lhs) || expr_opts_in(rhs),
        Expr::Not(inner) | Expr::Prove(inner) | Expr::Why(inner) | Expr::Audit(inner) => {
            expr_opts_in(inner)
        }
        Expr::Call { args, .. } => args.iter().any(expr_opts_in),
        Expr::Match {
            scrutinee, arms, ..
        } => expr_opts_in(scrutinee) || arms.iter().any(|arm| expr_opts_in(&arm.body)),
        _ => false,
    }
}

fn evidence_type_names(module: &Module) -> BTreeSet<String> {
    module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Newtype(decl) => Some(decl.name.clone()),
            Item::TypeDef(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect()
}

fn binding_owned_by_evidence(item: &Item, names: &BTreeSet<String>) -> bool {
    match item {
        Item::Let(binding) => {
            binding
                .ty
                .as_ref()
                .is_some_and(|ty| ty_mentions(ty, names) || ty_opts_in(ty))
                || expr_opts_in(&binding.value)
        }
        Item::Fn(fun) => {
            fun.requires.is_some()
                || fun.ensures.is_some()
                || fun.converts.is_some()
                || fun.params.iter().any(|param| {
                    param
                        .ty
                        .as_ref()
                        .is_some_and(|ty| ty_mentions(ty, names) || ty_opts_in(ty))
                })
                || fun
                    .ret
                    .as_ref()
                    .is_some_and(|ty| ty_mentions(ty, names) || ty_opts_in(ty))
        }
        _ => false,
    }
}

fn ty_mentions(ty: &ast::Ty, names: &BTreeSet<String>) -> bool {
    match ty {
        ast::Ty::Named(name) => names.contains(name),
        ast::Ty::App(name, args) => {
            names.contains(name) || args.iter().any(|arg| ty_mentions(arg, names))
        }
        ast::Ty::Graded(inner, _) | ast::Ty::Given { inner, .. } => ty_mentions(inner, names),
        ast::Ty::Refine { base, .. } => ty_mentions(base, names),
        ast::Ty::Record(fields) => fields.iter().any(|field| ty_mentions(&field.ty, names)),
        ast::Ty::Scalar(_) => false,
    }
}
