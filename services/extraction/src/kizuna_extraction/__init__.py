"""KIZUNA extraction worker: turns one untrusted document into label/value records.

Runs as a sandboxed subprocess (crates/kizuna-api/src/extraction.rs): bytes in on stdin, JSON on stdout.
No database credentials, no network, no access to other documents. Document text is data, never instructions.
"""

__version__ = "0.3.0"
