"""Strict one-action protocol shared by the model, host, and training data."""

from __future__ import annotations

from typing import Annotated, Literal, TypeAlias

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    TypeAdapter,
    field_validator,
    model_validator,
)


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class CandidateFile(StrictModel):
    path: str = Field(min_length=1)
    content: str

    @field_validator("path")
    @classmethod
    def relative_package_path(cls, path: str) -> str:
        if path.startswith(("/", "~")) or ".." in path.split("/"):
            raise ValueError("candidate paths must be package-relative")
        return path


class CandidateEdit(StrictModel):
    """One exact, auditable replacement inside an existing package file."""

    path: str = Field(min_length=1)
    old_text: str = Field(min_length=1)
    new_text: str

    @field_validator("path")
    @classmethod
    def relative_package_path(cls, path: str) -> str:
        return CandidateFile.relative_package_path(path)


class ExpectedChange(StrictModel):
    adds: list[str] = Field(default_factory=list)
    removes: list[str] = Field(default_factory=list)
    capability_changes: list[str] = Field(default_factory=list)
    ownership_changes: list[str] = Field(default_factory=list)


class ProjectContextAction(StrictModel):
    action: Literal["project_context"]
    reason: str


class FindAction(StrictModel):
    action: Literal["find"]
    query: str = Field(min_length=1)
    kinds: list[str] = Field(default_factory=list)
    limit: int = Field(default=20, ge=1, le=100)
    reason: str


class InspectAction(StrictModel):
    action: Literal["inspect"]
    subjects: list[str] = Field(min_length=1, max_length=20)
    reason: str


class ReadSourceAction(StrictModel):
    action: Literal["read_source"]
    path: str
    start_line: int = Field(default=1, ge=1)
    end_line: int = Field(ge=1)
    reason: str


class CandidateAction(StrictModel):
    reason: str


class CheckCandidateAction(CandidateAction):
    action: Literal["check_candidate"]


class FormatCandidateAction(CandidateAction):
    action: Literal["format_candidate"]


class TestCandidateAction(CandidateAction):
    action: Literal["test_candidate"]
    selectors: list[str] = Field(default_factory=list)


class QualityCandidateAction(CandidateAction):
    action: Literal["quality_candidate"]
    profile: Literal["standard", "production"] = "standard"


class DiffCandidateAction(CandidateAction):
    action: Literal["diff_candidate"]


class ImpactCandidateAction(CandidateAction):
    action: Literal["impact_candidate"]


class PackageBuildAction(CandidateAction):
    action: Literal["package_build"]


class ProposePatchAction(StrictModel):
    action: Literal["propose_patch"]
    files: list[CandidateFile] = Field(default_factory=list, max_length=32)
    edits: list[CandidateEdit] = Field(default_factory=list, max_length=32)
    expected_change: ExpectedChange
    required_validation: list[
        Literal["check", "format", "test", "quality", "diff", "impact", "package_build"]
    ] = Field(min_length=1)
    reason: str

    @model_validator(mode="after")
    def contains_a_change(self) -> "ProposePatchAction":
        if not self.files and not self.edits:
            raise ValueError("propose_patch requires at least one file or exact edit")
        return self


class FinishAction(StrictModel):
    action: Literal["finish"]
    status: Literal["validated_candidate", "needs_work", "blocked"]
    summary: str
    evidence_ids: list[str] = Field(default_factory=list)
    residual_obligations: list[str] = Field(default_factory=list)


Action: TypeAlias = Annotated[
    ProjectContextAction
    | FindAction
    | InspectAction
    | ReadSourceAction
    | CheckCandidateAction
    | FormatCandidateAction
    | TestCandidateAction
    | QualityCandidateAction
    | DiffCandidateAction
    | ImpactCandidateAction
    | PackageBuildAction
    | ProposePatchAction
    | FinishAction,
    Field(discriminator="action"),
]

ACTION_ADAPTER = TypeAdapter(Action)


def parse_action(raw: str) -> Action:
    """Validate a model emission as exactly one action."""

    return ACTION_ADAPTER.validate_json(raw)


def action_json_schema() -> dict:
    return ACTION_ADAPTER.json_schema()
