"""CostTracker — Python mirror of @wisent/cost-tracker."""

import atexit
from dataclasses import dataclass
import math
import signal
from pathlib import Path
from typing import Any, Dict, List, Optional

from .pricing import (
    captcha_price,
    compute_cost,
    llm_cost,
    proxy_cost_for_bytes,
    sms_price,
    PRICES,
)
from .sinks import CostSink, FileSink, MemorySink, SupabaseSink
from .types import CostRecord

_USAGE_TYPES = {"solves", "tokens", "bytes", "seconds", "units", "emails"}


@dataclass
class CostTrackerOptions:
    agent_id: str
    reference_id: Optional[str] = None
    sink: str = "memory"            # "memory" | "file" | "supabase"
    file_path: Optional[Path] = None
    supabase_url: Optional[str] = None
    supabase_key: Optional[str] = None
    auto_flush: bool = True


def _round4(n: float) -> float:
    return round(n, 4)


def _normalize_llm_service(model: str) -> str:
    m = model.lower()
    if "gemini" in m:
        return "gemini"
    if "claude" in m:
        return "claude"
    if "gpt" in m:
        return "openai"
    return "other"


class CostTracker:
    def __init__(self, opts: CostTrackerOptions | None = None, **kwargs: Any) -> None:
        if opts is None:
            opts = CostTrackerOptions(**kwargs)
        if not isinstance(opts.agent_id, str) or not opts.agent_id.strip():
            raise ValueError("CostTracker: agent_id must be a non-empty string")
        self._opts = opts
        self._buffer: List[CostRecord] = []
        self._flushed = False

        if opts.sink == "supabase":
            if not (opts.supabase_url and opts.supabase_key):
                raise ValueError("CostTracker: sink='supabase' requires supabase_url + supabase_key")
            self._sink: CostSink = SupabaseSink(opts.supabase_url, opts.supabase_key)
        elif opts.sink == "file":
            if not opts.file_path:
                raise ValueError("CostTracker: sink='file' requires file_path")
            self._sink = FileSink(opts.file_path)
        else:
            self._sink = MemorySink()

        if opts.auto_flush:
            atexit.register(self._flush_sync)
            for sig in (signal.SIGINT, signal.SIGTERM):
                try:
                    signal.signal(sig, self._signal_flush)
                except (ValueError, OSError):
                    # signal may not be settable in non-main threads
                    pass

    def _signal_flush(self, _signum: int, _frame: Any) -> None:
        self._flush_sync()

    def _flush_sync(self) -> None:
        try:
            self.flush()
        except Exception:  # noqa: BLE001
            pass

    def record(
        self,
        service: str,
        usage_type: str,
        usage_amount: float,
        cost_usd: float,
        resource: Optional[str] = None,
        reference_id: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> CostRecord:
        if not isinstance(service, str) or not service.strip():
            raise ValueError("CostTracker: service must be a non-empty string")
        if usage_type not in _USAGE_TYPES:
            raise ValueError(f"CostTracker: unsupported usage_type {usage_type!r}")
        if type(usage_amount) not in (int, float) or not math.isfinite(usage_amount) or usage_amount <= 0:
            raise ValueError("CostTracker: usage_amount must be a finite number greater than zero")
        if type(cost_usd) not in (int, float) or not math.isfinite(cost_usd) or cost_usd < 0:
            raise ValueError("CostTracker: cost_usd must be a finite non-negative number")
        rec = CostRecord(
            service=service,
            resource=resource,
            usage_type=usage_type,  # type: ignore[arg-type]
            usage_amount=float(usage_amount),
            cost_usd=_round4(cost_usd),
            reference_id=reference_id or self._opts.reference_id,
            metadata=metadata or {},
            created_at=datetime.utcnow().isoformat() + "Z",
            agent_id=self._opts.agent_id,
        )
        self._buffer.append(rec)
        return rec

    def record_captcha(self, service: str, task_type: str, override: Optional[float] = None) -> CostRecord:
        return self.record(
            service=f"captcha_{service}",
            resource=task_type,
            usage_type="solves",
            usage_amount=1,
            cost_usd=override if override is not None else captcha_price(service, task_type),
        )

    def record_sms(self, provider: str, platform: str, override: Optional[float] = None) -> CostRecord:
        return self.record(
            service=f"sms_{provider}",
            resource=platform,
            usage_type="units",
            usage_amount=1,
            cost_usd=override if override is not None else sms_price(provider, platform),
        )

    def record_proxy_bytes(self, provider: str, num_bytes: int, is_mobile: bool = False) -> CostRecord:
        key = "oxylabs_mobile" if (is_mobile and provider == "oxylabs") else provider
        return self.record(
            service=f"proxy_{key}",
            resource="mobile" if is_mobile else "residential",
            usage_type="bytes",
            usage_amount=num_bytes,
            cost_usd=proxy_cost_for_bytes(provider, num_bytes, is_mobile),
        )

    def record_llm(
        self,
        model: str,
        input_tokens: int = 0,
        output_tokens: int = 0,
        override: Optional[float] = None,
        skill_id: Optional[str] = None,
    ) -> CostRecord:
        meta: Dict[str, Any] = {"input_tokens": input_tokens, "output_tokens": output_tokens}
        if skill_id:
            meta["skill_id"] = skill_id
        return self.record(
            service=f"llm_{_normalize_llm_service(model)}",
            resource=model,
            usage_type="tokens",
            usage_amount=input_tokens + output_tokens,
            cost_usd=override if override is not None else llm_cost(model, input_tokens, output_tokens),
            metadata=meta,
        )

    def record_compute(self, instance_type: str, seconds: float, override: Optional[float] = None) -> CostRecord:
        return self.record(
            service=f"compute_{instance_type.split('_')[0]}",
            resource=instance_type,
            usage_type="seconds",
            usage_amount=seconds,
            cost_usd=override if override is not None else compute_cost(instance_type, seconds),
        )

    def record_email(self, provider: str, count: int = 1, override: Optional[float] = None) -> CostRecord:
        unit = PRICES["email"].get(provider, PRICES["email"]["default"])
        return self.record(
            service=f"email_{provider}",
            resource=provider,
            usage_type="emails",
            usage_amount=count,
            cost_usd=override if override is not None else unit * count,
        )

    def total(self) -> float:
        return _round4(sum(r.cost_usd for r in self._buffer))

    def snapshot(self) -> Dict[str, Any]:
        service_costs: Dict[str, float] = {}
        for r in self._buffer:
            service_costs[r.service] = _round4(service_costs.get(r.service, 0) + r.cost_usd)
        return {
            "cost_usd": self.total(),
            "service_costs": service_costs,
            "records": [r.to_json() for r in self._buffer],
        }

    def flush(self) -> None:
        if self._flushed or not self._buffer:
            self._flushed = True
            return
        self._sink.write(self._buffer)
        try:
            from .onboarding import observe_accepted_usage

            observe_accepted_usage(self._buffer)
        except (OSError, TypeError, ValueError):
            # First-use telemetry must never alter an accepted sink write.
            pass
        self._flushed = True

    def get_sink(self) -> CostSink:
        return self._sink
