# Reference Python client

A standard-library-only Python 3 client for `rusty_multimodal_db`'s
server/query layer, written against `SERVER-002` (the wire specification)
at protocol version 12. It exists to prove that specification is
sufficient (`ECO-FR-007`–`009`, ADR-0043); it is not a packaged product.

```python
import uuid
from rusty_multimodal_db import Client, CompareOp

with Client.connect("127.0.0.1", 7878) as c:
    c.schema.fields                    # what DescribeSchema reported
    c.get(uuid.UUID(int=1))            # [("label", "Ada Lovelace"), ...] or None
    c.filter_eq("label", "ada")        # ids, via the server's name index
    c.query(["label"], where=[("kind", CompareOp.Eq, "person")])
    c.join("relates_to", ["label"], ["label"])   # protocol 12
```

Verification, both in CI:

- offline: `python3 -m unittest discover -s clients/python/tests -v` —
  every line of `tests/fixtures/wire-vectors.txt` decodes and re-encodes
  byte-for-byte;
- live: `tests/server_python_client.rs` (under `cargo test
  --all-features`) starts a real `Entity` server and runs `driver.py`
  against it, at protocol 12 and at a hand-negotiated 10.

The client is version-pinned: when the wire grows, the fixture and
`SERVER-002` grow in the same change; this client is updated when someone
wants the new shape and stays correct at the version it declares.
