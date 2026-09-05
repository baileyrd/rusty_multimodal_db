"""ECO-FR-008 (i), offline: every line of tests/fixtures/wire-vectors.txt
at or below this client's declared protocol version decodes and
re-encodes byte-for-byte. No server, no Rust toolchain."""

import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

from rusty_multimodal_db import protocol as p  # noqa: E402

FIXTURE = os.path.join(HERE, "..", "..", "..", "tests", "fixtures", "wire-vectors.txt")


def load_vectors():
    with open(FIXTURE, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            name, version, hexbytes = line.split("\t")
            yield name, int(version), bytes.fromhex(hexbytes)


class WireVectors(unittest.TestCase):
    def test_fixture_exists_and_is_not_empty(self):
        vectors = list(load_vectors())
        self.assertGreater(len(vectors), 40)
        self.assertTrue(any(n.startswith("Request/") for n, _, _ in vectors))
        self.assertTrue(any(n.startswith("Response/") for n, _, _ in vectors))

    def test_every_request_vector_round_trips(self):
        for name, version, data in load_vectors():
            if not name.startswith("Request/") or version > p.PROTOCOL_VERSION:
                continue
            with self.subTest(vector=name):
                value = p.decode_request(data)
                self.assertEqual(p.encode_request(value), data)

    def test_every_response_vector_round_trips(self):
        for name, version, data in load_vectors():
            if not name.startswith("Response/") or version > p.PROTOCOL_VERSION:
                continue
            with self.subTest(vector=name):
                value = p.decode_response(data)
                self.assertEqual(p.encode_response(value), data)

    def test_no_vector_is_above_the_declared_version(self):
        # The fixture and this client are pinned to the same version; a
        # newer fixture is a signal to update REQUEST_INTRODUCED_AT & co.
        for name, version, _ in load_vectors():
            with self.subTest(vector=name):
                self.assertLessEqual(version, p.PROTOCOL_VERSION)

    def test_handshake_frames_match_the_specification_examples(self):
        # SERVER-002 §4's worked examples: Hello { 12 } and GetById(uuid 1).
        import uuid

        self.assertEqual(p.encode_request(p.Hello(12)), bytes.fromhex("0a0000000c000000"))
        self.assertEqual(
            p.frame(p.encode_request(p.GetById(uuid.UUID(int=1)))),
            bytes.fromhex("1c000000" "00000000" "1000000000000000" + "00" * 15 + "01"),
        )


if __name__ == "__main__":
    unittest.main()
