# Policy Files

Policy configuration is passed through `--policies-json` as inline JSON or a
path to a JSON file. That configuration defines one or more policy chains for
fetch and module evaluation, and it also controls optional filesystem
operations when those capabilities are enabled.

The reference should document the configuration shape, supported keys, and the
kinds of input each policy receives, not just example workflows.
