from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from harness.common import Control


@dataclass
class Observation:
    output: str
    status: str = "completed"
    tools: list[dict] = field(default_factory=list)
    tool_events_complete: bool = False
    metrics: dict = field(default_factory=dict)


@dataclass
class Context:
    workspace: Path
    state: Path
    home: Path
    control: Control
    emit: Callable[[str, str], None]
    gateway: object
    agent: dict
    model: dict


class Adapter:
    def __init__(self, context: Context):
        self.context = context

    def start(self) -> None:
        pass

    def turn(self, prompt: str) -> Observation:
        raise NotImplementedError

    def restart(self) -> None:
        raise NotImplementedError

    def close(self) -> None:
        pass
