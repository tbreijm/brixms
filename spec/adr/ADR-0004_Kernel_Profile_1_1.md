# ADR-0004 — Proof Kernel Profile 1.1: Realization Composition Rule

Status: **Proposed** (2026-07-29) (extends [ADR-0003](./ADR-0003_Proof_Kernel_Profile.md) §5; governs `brix-kernel`).

Date: 2026-07-29.

Foundation documents: [ADR-0003: Proof Kernel Profile](./ADR-0003_Proof_Kernel_Profile.md), [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5, §10 PD-1). This ADR defines Profile 1.1 of `brix-kernel`, adding the realization-composition inference rule needed to certify settlement decomposition-validity.

---

## 1. Decision & Rule Definition

Profile 1.1 adds exactly **ONE** inference rule, `RealizesComp`, to `brix-kernel`. This rule enables extensional proof-kernel certification of generator chain composition $k = g_n \circ \dots \circ g_1$, fulfilling the first theorem target (Decomposition-Validity) from ADR-0003 §7.

### 1.1 Judgement Rule

$$\frac{\Gamma \vdash p : \mathsf{Realizes}(g_1, x, y) \quad \Gamma \vdash q : \mathsf{Realizes}(g_2, y, z)}{\Gamma \vdash \mathsf{realizes\_comp}(p, q) : \mathsf{Realizes}(\mathsf{compose}(g_2, g_1), x, z)} \quad (\text{RealizesComp})$$

---

## 2. Soundness & Epistemic Scope

### 2.1 Theoretical Justification

This rule is the proof-theoretic realization of SOC's sound-path-compression corollary and lax-composition axiom:

$$\rho_{g \circ f} \supseteq \rho_g \circ \rho_f$$

(per `docs/SOC_core_foundations_revised.tex`). A composite generator $g_2 \circ g_1$ realizes at least all state transition possibilities realized by sequential execution of $g_1$ followed by $g_2$. Therefore, chaining valid realization evidence $(p, q)$ soundly entails realization of the composite transition $(x \to z)$ under $\mathsf{compose}(g_2, g_1)$.

### 2.2 Lax vs. Tight Realization Scope

- **Lax Direction (Proven Here):** $\mathsf{RealizesComp}$ proves that $\mathsf{compose}(g_2, g_1)$ realizes at least the outcome pair $(x, z)$ witnessed by the chain.
- **Tight Direction (Deferred):** The converse assertion—that $k$ realizes *nothing beyond* the composite chain—depends on $\mathcal{G}$-tightness ([ADR-0002](./ADR-0002_SOC_Constitution.md) §10 PD-1) and is **NOT** claimed by this rule.

---

## 3. Mandatory Side Conditions

Every evaluation of $\mathsf{realizes\_comp}(p, q)$ against an expected proposition $\mathsf{Realizes}(w, x, z)$ MUST strictly verify three mandatory side conditions:

1. **Middle Endpoint Match:** The target endpoint $y$ of $p$'s type $\mathsf{Realizes}(g_1, x_l, y)$ and the source endpoint $y_s$ of $q$'s type $\mathsf{Realizes}(g_2, y_s, z_r)$ MUST be **canonically equal** ($y \equiv y_s$). Any mismatch MUST immediately result in `Verdict::Rejected` (chaining non-adjacent settlement transitions is logically unsound).
2. **Witness Structure Match:** The goal proposition's witness $w$ MUST be canonically equal to $\mathsf{compose}(g_2, g_1)$ (outer $g_2$, inner $g_1$).
3. **Outer Endpoint Match:** The goal's source endpoint $x$ MUST canonically equal $x_l$, and the goal's target endpoint $z$ MUST canonically equal $z_r$.

---

## 4. Canonical Encoding & Ordinals

To maintain backward compatibility and binary revision invariance, all newly introduced constructs use append-only canonical ordinals. Existing frozen ordinals remain unchanged.

### 4.1 ObjectTerm Additions

- `ObjectTerm::Compose(Box<ObjectTerm>, Box<ObjectTerm>)`: Canonical ordinal **2** (append-only after `BoundVar = 1`).

### 4.2 TermKind Additions

- `TermKind::RealizesComp { left: Box<TermKind>, right: Box<TermKind> }`: Canonical ordinal **15** (append-only after `Pres = 14`).

---

## 5. Non-goals

- No change to existing frozen canonical ordinals 0–14 in `TermKind` or 0–1 in `ObjectTerm`.
- No introduction of normalization, β-reduction, or unification during endpoint comparison. Canonical structural equality is enforced directly.
