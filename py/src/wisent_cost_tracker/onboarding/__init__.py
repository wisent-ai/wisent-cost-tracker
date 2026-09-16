"""Durable machine first-use for wisent-cost-tracker.

Four parts, in the order they depend on each other:

* ``definition`` - what the product published: the journey identity, the
  definition itself and the check that a bundle really is it.
* ``engine`` - one machine's attempt: the state file, the queue of events and
  the control plane they go to.
* ``observations`` - the three points where real work advances the journey.
* ``actions`` - what an operator can ask for, and the command line.

The command ``wisent-cost-tracker-onboarding`` enters at :func:`main`.
"""

from .actions import ACTIONS, main, run_onboarding_action
from .definition import (
    CANONICAL_DEFINITION,
    CANONICAL_EVENTS,
    FIRST_SUCCESS_FACT,
    JOURNEY_ID,
    JOURNEY_VERSION,
    JOURNEY_VERSION_ID,
    PRODUCT_ID,
    SOURCE_REVISION,
    OnboardingError,
)
from .observations import (
    observe_accepted_usage,
    observe_budget_decision,
    observe_budget_status,
)

__all__ = [
    "ACTIONS",
    "CANONICAL_DEFINITION",
    "CANONICAL_EVENTS",
    "FIRST_SUCCESS_FACT",
    "JOURNEY_ID",
    "JOURNEY_VERSION",
    "JOURNEY_VERSION_ID",
    "OnboardingError",
    "PRODUCT_ID",
    "SOURCE_REVISION",
    "main",
    "observe_accepted_usage",
    "observe_budget_decision",
    "observe_budget_status",
    "run_onboarding_action",
]
