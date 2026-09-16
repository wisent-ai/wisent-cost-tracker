"""The first-use control plane, and the events posted to it.

Every call goes through :meth:`StadoTransport.post`. The first failure marks
the transport unavailable for the rest of the process, so an unreachable
control plane costs one timeout rather than one per operation, and the caller
sees a ``RuntimeError`` naming the operation instead of a partial result.

The timeout default is short on purpose: first-use telemetry must never hold
up a budget read. The operator sets STADO_ONBOARDING_TIMEOUT_SECONDS to change
it.

An event is built here and sent here, but it is queued on disk by the runtime
first: :func:`flush` only drops an event from that queue once the control
plane has taken it, which is what lets a journey happen on a machine that is
offline while it does.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.parse
import urllib.request
import uuid
from typing import Any, Callable, Mapping, MutableSequence

from ..definition import (
    CANONICAL_EVENTS,
    CLIENT_ID,
    JOURNEY_ID,
    JOURNEY_VERSION,
    JOURNEY_VERSION_ID,
    PRODUCT_ID,
    now,
)


class StadoTransport:
    def __init__(self) -> None:
        self.base_url = os.environ.get("STADO_INTEGRATION_API_URL", "").rstrip("/")
        self.token = os.environ.get("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN", "")
        self.timeout = float(os.environ.get("STADO_ONBOARDING_TIMEOUT_SECONDS", "2"))
        self.available = True

    def post(self, operation: str, body: Mapping[str, Any]) -> dict[str, Any]:
        if not self.available or not self.base_url or not self.token:
            raise RuntimeError("onboarding control plane is unavailable")
        parsed = urllib.parse.urlparse(self.base_url)
        if parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password:
            self.available = False
            raise RuntimeError("onboarding control plane URL is invalid")
        request = urllib.request.Request(
            f"{self.base_url}/api/integration/onboarding/{operation}",
            data=json.dumps({"client_id": CLIENT_ID, **body}, separators=(",", ":")).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "application/json",
                "X-Onboarding-Client": CLIENT_ID,
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                envelope = json.loads(response.read().decode("utf-8"))
            if not isinstance(envelope, dict):
                raise ValueError("invalid response")
            if envelope.get("ok") is True and "result" in envelope:
                result = envelope["result"]
                return dict(result) if isinstance(result, Mapping) else {}
            return envelope
        except (OSError, urllib.error.URLError, ValueError, json.JSONDecodeError) as error:
            self.available = False
            raise RuntimeError(f"Stado {operation} unavailable") from error

    def read_bundle(self) -> dict[str, Any]:
        return self.post(
            "bundle.read",
            {
                "product_id": PRODUCT_ID,
                "journey_id": JOURNEY_ID,
                "journey_version": JOURNEY_VERSION,
                "if_none_match": None,
            },
        )

    def assign(self, subject_hash: str) -> dict[str, Any]:
        return self.post(
            "experiments.assign",
            {
                "product_id": PRODUCT_ID,
                "journey_id": JOURNEY_ID,
                "journey_version": JOURNEY_VERSION,
                "subject_hash": subject_hash,
                "scope_kind": "device",
                "surface": "sdk_first_use",
            },
        )

    def read_state(self, progress: Mapping[str, Any]) -> dict[str, Any]:
        return self.post(
            "state.read",
            {
                "product_id": PRODUCT_ID,
                "journey_version_id": JOURNEY_VERSION_ID,
                "attempt_id": progress["attempt_id"],
                "subject_hash": progress["subject_hash"],
                "scope_kind": "device",
            },
        )

    def collect(self, event: Mapping[str, Any]) -> dict[str, Any]:
        return self.post("events.collect", event)


def build_event(
    name: str,
    *,
    progress: Mapping[str, Any],
    subject_hash: str,
    properties: Mapping[str, Any] | None = None,
    screen_id: str | None = None,
    next_screen_id: str | None = None,
    reason_code: str | None = None,
) -> dict[str, Any]:
    """One analytics event, in the shape the contract names.

    An event name outside the published set is a programming error and raises,
    rather than being posted and rejected at the far end.
    """
    if name not in CANONICAL_EVENTS:
        raise ValueError(f"unsupported onboarding event {name}")
    assignment = progress.get("assignment", {})
    return {
        "event_id": str(uuid.uuid4()),
        "event_name": name,
        "attempt_id": progress["attempt_id"],
        "product_id": PRODUCT_ID,
        "journey_id": JOURNEY_ID,
        "journey_version": JOURNEY_VERSION,
        "journey_version_id": JOURNEY_VERSION_ID,
        "subject_hash": subject_hash,
        "scope_kind": "device",
        "screen_id": screen_id or progress["current_screen_id"],
        "occurred_at": now(),
        "evidence_revision": progress["evidence_revision"],
        "experiment_id": assignment.get("experiment_id"),
        "variant_id": assignment.get("variant_id", "control"),
        "selected_next_screen_id": next_screen_id,
        "reason_code": reason_code,
        "properties": dict(properties or {}),
        "answers": [],
    }


def flush(
    pending: MutableSequence[dict[str, Any]],
    transport: StadoTransport,
    persist: Callable[[], None],
) -> None:
    """Hand queued events over in order, keeping whatever is not taken.

    The queue is persisted after each accepted event, so a process that dies
    mid-flush neither loses an event nor sends one twice.
    """
    while pending:
        try:
            transport.collect(pending[0])
        except RuntimeError:
            return
        pending.pop(0)
        persist()
