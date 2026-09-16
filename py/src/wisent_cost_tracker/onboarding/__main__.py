"""`python -m wisent_cost_tracker.onboarding`, the same entry as the command.

The installed console script wisent-cost-tracker-onboarding calls the same
main, so both ways of running it answer identically.
"""

from .actions import main

raise SystemExit(main())
