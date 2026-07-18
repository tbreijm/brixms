"""Configuration with conservative local-only defaults."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class BuilderConfig:
    root: Path
    brix_binary: Path
    model: str = "mlx-community/Qwen3.5-4B-MLX-4bit"
    endpoint: str = "http://127.0.0.1:8080/v1"
    context_tokens: int = 8192
    max_actions: int = 12
    repair_rounds: int = 3
    request_timeout_seconds: int = 180

    def normalized(self) -> "BuilderConfig":
        root = self.root.expanduser().resolve()
        brix = self.brix_binary.expanduser()
        if not brix.is_absolute():
            brix = (Path.cwd() / brix).resolve()
        return BuilderConfig(
            root=root,
            brix_binary=brix,
            model=self.model,
            endpoint=self.endpoint.rstrip("/"),
            context_tokens=self.context_tokens,
            max_actions=self.max_actions,
            repair_rounds=self.repair_rounds,
            request_timeout_seconds=self.request_timeout_seconds,
        )
