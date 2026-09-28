"""A pure standard-library Python client for `brix serve --stdio` (ADR-0044).

No compiled extension and no third-party dependency: this package only uses
`subprocess`, `json`, `threading`, and `queue` from the standard library, and
targets Python >= 3.9.

Typical usage::

    from brix import BrixClient

    with BrixClient() as client:
        result = client.run(program={"path": "examples/shipping.brix"})
        print(result["status"], result["decision"])

See ``README.md`` in this directory for the full protocol summary and more
examples.
"""

from .client import (
    BrixClient,
    BrixError,
    BrixProcessError,
    BrixProtocolError,
    BrixTimeoutError,
    ProgramSpec,
    path,
    source,
)

__all__ = [
    "BrixClient",
    "BrixError",
    "BrixProcessError",
    "BrixProtocolError",
    "BrixTimeoutError",
    "ProgramSpec",
    "path",
    "source",
]

__version__ = "0.1.0"
