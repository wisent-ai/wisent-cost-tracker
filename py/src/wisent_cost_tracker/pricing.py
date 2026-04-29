"""Pricing table loader. Reads `pricing/costs.json` packaged with the
distribution (or, for in-source dev, the canonical file at the repo root).

The packaged copy is created during build by setuptools via the package_data
declaration in pyproject.toml; for source installs we walk up the tree.
"""

import json
from pathlib import Path
from typing import Any, Dict

_cached: Dict[str, Any] | None = None


def _resolve_pricing_path() -> Path:
    here = Path(__file__).resolve().parent
    # Packaged location: src/wisent_cost_tracker/pricing/costs.json
    packaged = here / "pricing" / "costs.json"
    if packaged.exists():
        return packaged
    # Source-tree location: walk up to repo root, then into pricing/
    for parent in here.parents:
        candidate = parent / "pricing" / "costs.json"
        if candidate.exists():
            return candidate
    raise FileNotFoundError("Could not locate pricing/costs.json")


def load_pricing() -> Dict[str, Any]:
    global _cached
    if _cached is None:
        _cached = json.loads(_resolve_pricing_path().read_text("utf-8"))
    return _cached


PRICES: Dict[str, Any] = load_pricing()


def captcha_price(service: str, task_type: str) -> float:
    tbl = PRICES["captcha"].get(service, {})
    return tbl.get(task_type, tbl.get("default", 0.001))


def sms_price(service: str, platform: str) -> float:
    tbl = PRICES["sms"].get(service, {})
    return tbl.get(platform.lower(), tbl.get("default", 0.30))


def proxy_cost_for_bytes(provider: str, num_bytes: int, is_mobile: bool = False) -> float:
    key = "oxylabs_mobile" if (is_mobile and provider == "oxylabs") else provider
    per_gb = PRICES["proxy_per_gb"].get(key, PRICES["proxy_per_gb"]["default"])
    return (num_bytes / (1024 * 1024 * 1024)) * per_gb


def llm_cost(model: str, input_tokens: int, output_tokens: int) -> float:
    ml = model.lower()
    prices = PRICES["llm"]["default"]
    for key, value in PRICES["llm"].items():
        if key != "default" and key.lower() in ml:
            prices = value
            break
    return (input_tokens / 1000) * prices["input_per_1k"] + (output_tokens / 1000) * prices["output_per_1k"]


def compute_cost(instance_type: str, seconds: float) -> float:
    per_hour = PRICES["compute_per_hour"].get(instance_type, PRICES["compute_per_hour"]["default"])
    return (seconds / 3600) * per_hour
