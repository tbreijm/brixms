//! Bounded inspection/admission of the shared context model; never publishes a theorem.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_semantic::{
    decode_context_model_v2, validate_context_model_v2, ArtifactRefV2, AssumptionValidatorV2,
    ContextId, ContextResolverV2, ModelLimits, ScenarioModeV2,
};
use serde_json::{json, Value};

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let limit = ModelLimits::default().bytes;
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("context input unavailable: {e}"))?;
    if !metadata.is_file() {
        return Err("context input must be a regular file".into());
    }
    if metadata.len() > limit as u64 {
        return Err("context input exceeds byte budget".into());
    }
    let file = File::open(path).map_err(|e| format!("context input unavailable: {e}"))?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("context input exceeds byte budget".into());
    }
    Ok(bytes)
}

struct DirectoryResolver(Option<PathBuf>);
impl ContextResolverV2 for DirectoryResolver {
    fn context_bytes(&self, id: &ContextId) -> Option<Vec<u8>> {
        read_bounded(&self.0.as_ref()?.join(format!("{}.context", id.to_hex()))).ok()
    }
    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>> {
        // Paths use only the digest; untrusted artifact kinds cannot select paths.
        read_bounded(
            &self
                .0
                .as_ref()?
                .join(format!("{}.artifact", reference.digest.to_hex())),
        )
        .ok()
    }
}

struct Validator<'a>(&'a str);
impl AssumptionValidatorV2 for Validator<'_> {
    fn validate(&self, schema: &str, value: &[u8], budget: &mut usize) -> bool {
        match self.0 {
            "typing" => soc_regimes::context_validation::ClosedTyAssumptionValidator
                .validate(schema, value, budget),
            "kernel" => brix_kernel::KernelAssumptionValidator.validate(schema, value, budget),
            _ => false,
        }
    }
}

fn digest_hex(value: &str) -> Result<Digest, String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected context must be a 64-character hex digest".into());
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(Digest::from_bytes(bytes))
}

fn inspect(
    file: &Path,
    expect: Option<&str>,
    store: Option<&Path>,
    profile: &str,
    simulation: bool,
) -> Result<Value, String> {
    let bytes = read_bounded(file)?;
    let model = decode_context_model_v2(&bytes, ModelLimits::default())
        .map_err(|e| format!("context decoding: {e:?}"))?;
    let d = model.dimensions();
    let id = model.id();
    let mut validated = false;
    if let Some(expected) = expect {
        let expected = ContextId(digest_hex(expected)?);
        if id != expected {
            return Err(format!(
                "context identity mismatch: expected {}, observed {}",
                expected.to_hex(),
                id.to_hex()
            ));
        }
        let wanted_mode = if simulation {
            ScenarioModeV2::Simulation
        } else {
            ScenarioModeV2::Production
        };
        let regime = if profile == "typing" {
            "brix.type@1"
        } else {
            "brix.proof@1"
        };
        if d.scenario_mode != wanted_mode
            || d.kernel_profile != brix_kernel::KERNEL_PROFILE_V1
            || d.regime_profile != regime
            || d.axiom_profile != "none"
        {
            return Err(format!("context profile/mode mismatch at {}", id.to_hex()));
        }
        // The independently supplied expected ID binds every dimension. The
        // decoded dimensions are not a second, self-asserted trust anchor.
        validate_context_model_v2(
            &bytes,
            expected,
            d,
            ModelLimits::default(),
            &DirectoryResolver(store.map(Path::to_path_buf)),
            &Validator(profile),
        )
        .map_err(|e| format!("context {} rejected: {e:?}", id.to_hex()))?;
        validated = true;
    }
    Ok(json!({
        "schema":"brix.context.inspection@2", "ok":true,
        "status":if validated {"validated"} else {"decoded-only"},
        "context":id.to_hex(), "expected_context":expect,
        "authority":"none", "parent":d.parent.map(|id|id.to_hex()),
        "dimensions":{
            "program":d.program.as_ref().map(|r|json!({"kind":r.kind,"digest":r.digest.to_hex()})),
            "world":d.world.as_ref().map(|r|json!({"kind":r.kind,"digest":r.digest.to_hex()})),
            "namespace":d.namespace,"tenant":d.tenant,"principal":d.principal,
            "capability_view":d.capability_view,"scenario":format!("{:?}",d.scenario_mode),
            "branch":d.branch.map(|x|x.to_hex()),"valid_time":[d.valid_time_start,d.valid_time_end],
            "transaction_time":d.transaction_time,"correction_time":d.correction_time,"simulation_time":d.simulation_time,
            "observation_profile":d.observation_profile,"redaction_profile":d.redaction_profile,
            "kernel_profile":d.kernel_profile,"regime_profile":d.regime_profile,"axiom_profile":d.axiom_profile,
            "validity":[d.validity_start,d.validity_end],"steps":d.step_budget,"depth":d.depth_budget
        },
        "assumptions":model.assumptions().iter().map(|a|json!({"name":a.name,"schema":a.schema})).collect::<Vec<_>>()
    }))
}

pub fn execute(
    file: &Path,
    expect: Option<&str>,
    store: Option<&Path>,
    profile: &str,
    simulation: bool,
    json_output: bool,
) -> u8 {
    let (result, code) = match inspect(file, expect, store, profile, simulation) {
        Ok(result) => (result, crate::cli::EXIT_SUCCESS),
        Err(message) => (
            json!({"schema":"brix.context.inspection@2","ok":false,"status":"rejected","expected_context":expect,"authority":"none","diagnostics":[message]}),
            crate::cli::EXIT_REJECTED_OR_UNKNOWN,
        ),
    };
    if json_output {
        crate::json::emit_result_json(&result);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("JSON value")
        );
    }
    code
}
