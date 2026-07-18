"""Local model backends: in-process MLX or an MLX OpenAI-compatible server."""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from abc import ABC, abstractmethod
from collections.abc import Iterable
from typing import Any
from urllib.parse import urlparse


Message = dict[str, str]


class ModelError(RuntimeError):
    pass


class ModelBackend(ABC):
    @abstractmethod
    def complete(self, messages: list[Message]) -> str:
        """Return one JSON action and no surrounding prose."""


class DirectMlxBackend(ModelBackend):
    """Load one MLX model lazily and reuse it for every team role."""

    def __init__(
        self, model: str, adapter_path: str | None = None, max_tokens: int = 2048
    ):
        self.model_name = model
        self.adapter_path = adapter_path
        self.max_tokens = max_tokens
        self._model: Any = None
        self._tokenizer: Any = None

    def _load(self) -> None:
        if self._model is not None:
            return
        try:
            from mlx_lm import load
        except ImportError as error:
            raise ModelError(
                "mlx-lm is not installed; install brix-builder[mlx]"
            ) from error
        kwargs = {"adapter_path": self.adapter_path} if self.adapter_path else {}
        self._model, self._tokenizer = load(self.model_name, **kwargs)

    def complete(self, messages: list[Message]) -> str:
        self._load()
        try:
            from mlx_lm import generate

            prompt = self._tokenizer.apply_chat_template(
                messages,
                tokenize=False,
                add_generation_prompt=True,
                enable_thinking=False,
            )
            return generate(
                self._model,
                self._tokenizer,
                prompt=prompt,
                max_tokens=self.max_tokens,
                verbose=False,
            ).strip()
        except Exception as error:  # MLX surfaces backend-specific exception types.
            raise ModelError(f"MLX generation failed: {error}") from error


class LocalServerBackend(ModelBackend):
    """Call an OpenAI-compatible endpoint, refusing non-local hosts."""

    def __init__(
        self,
        endpoint: str,
        model: str,
        timeout_seconds: int = 180,
        max_tokens: int = 2048,
    ):
        endpoint = endpoint.rstrip("/")
        parsed = urlparse(endpoint)
        if parsed.scheme not in {"http", "https"} or parsed.hostname not in {
            "127.0.0.1",
            "localhost",
            "::1",
        }:
            raise ModelError("BrixBuilder server endpoints must be local")
        self.endpoint = endpoint
        self.model = model
        self.timeout_seconds = timeout_seconds
        self.max_tokens = max_tokens

    def complete(self, messages: list[Message]) -> str:
        body = json.dumps(
            {
                "model": self.model,
                "messages": messages,
                "temperature": 0.1,
                "max_tokens": self.max_tokens,
                "chat_template_kwargs": {"enable_thinking": False},
            }
        ).encode()
        request = urllib.request.Request(
            f"{self.endpoint}/chat/completions",
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(
                request, timeout=self.timeout_seconds
            ) as response:
                payload = json.load(response)
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            raise ModelError(f"local model request failed: {error}") from error
        try:
            return payload["choices"][0]["message"]["content"].strip()
        except (KeyError, IndexError, TypeError, AttributeError) as error:
            raise ModelError(
                "local model returned an invalid chat-completions response"
            ) from error


class ScriptedBackend(ModelBackend):
    """Deterministic backend for orchestration tests and replays."""

    def __init__(self, actions: Iterable[str]):
        self.actions = iter(actions)
        self.messages: list[list[Message]] = []

    def complete(self, messages: list[Message]) -> str:
        self.messages.append(list(messages))
        try:
            return next(self.actions)
        except StopIteration as error:
            raise ModelError("scripted backend exhausted") from error
