"""ECO-FR-008 (ii): the live driver tests/server_python_client.rs runs.

Usage: python3 driver.py HOST:PORT HELLO_VERSION

Connects to an Entity server, performs one of each read shape plus one
write, and prints ``key=value`` lines the Rust test asserts on. Nothing
here is a library API; it exists so a Rust test can prove the Python
client against a real server without parsing JSON.
"""

import sys
import uuid

sys.path.insert(0, __file__.rsplit("/", 1)[0])

from rusty_multimodal_db import Client, AggregateFn, CompareOp, UnsupportedError  # noqa: E402


def main() -> int:
    host, port = sys.argv[1].rsplit(":", 1)
    hello = int(sys.argv[2])
    with Client.connect(host, int(port), protocol_version=hello) as c:
        print(f"negotiated={c.server_protocol_version}")
        print("schema_fields=" + ",".join(f.name for f in c.schema.fields))
        print("relations=" + ",".join(sorted(r.name for r in c.relations)))

        ada = c.get(uuid.UUID(int=1))
        print(f"get_fields={len(ada)}")
        aliases = dict(ada).get("aliases")
        print("get_aliases=" + ("|".join(aliases) if aliases is not None else "-"))
        print(f"get_missing={'none' if c.get(uuid.UUID(int=99)) is None else 'found'}")

        ids = c.filter_eq("label", "ADA")
        print(f"filter_eq_ids={len(ids)}")
        print(f"filter_eq_first={ids[0].int if ids else '-'}")

        rows = c.query(["label"], where=[("kind", CompareOp.Eq, "person")])
        print(f"query_rows={len(rows)}")

        groups = c.aggregate(["kind"], [(AggregateFn.Count, None)])
        print(f"aggregate_groups={len(groups)}")

        print(f"update={str(c.update(uuid.UUID(int=1), 'mention_count', 42)).lower()}")
        try:
            c.update(uuid.UUID(int=1), "label", "x")
            print("update_label=allowed")
        except UnsupportedError:
            print("update_label=unsupported")

        print(f"neighbors={len(c.neighbors(uuid.UUID(int=1)))}")

        try:
            joined = c.join("relates_to", ["label"], ["label"])
            print(f"join_rows={len(joined)}")
        except UnsupportedError as e:
            print(f"join_rows=unsupported:{e}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
