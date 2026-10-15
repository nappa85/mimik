"""Optional synthetic evidence-structure prototype; not a PDF extractor or app store."""

import copy
import hashlib
import json

from jsonschema import Draft202012Validator


def record(properties, required=None):
    return {
        "type": "object",
        "properties": properties,
        "required": required or list(properties),
        "additionalProperties": False,
    }


ID = {"type": "string", "pattern": "^[A-Za-z0-9_.-]{1,80}$"}
TEXT = {"type": "string", "maxLength": 20000}
POINT = {
    "type": ["array", "null"],
    "prefixItems": [
        {"type": "integer", "minimum": 1},
        {"type": "integer", "minimum": 0, "maximum": 10000000},
    ],
    "minItems": 2,
    "maxItems": 2,
}
REFERENCE = record({"book": ID, "revision": ID, "node": ID})
NODE = record(
    {
        "id": ID,
        "title": TEXT,
        "parent_state": {"enum": ["root", "known", "unresolved"]},
        "parent": {"anyOf": [ID, {"type": "null"}]},
        "start": POINT,
        "end": POINT,
        "text": TEXT,
        "references": {"type": "array", "items": REFERENCE, "maxItems": 64},
        "reason": TEXT,
    },
    ["id", "title", "parent_state", "parent", "start", "end", "text", "references"],
)
BOOK = record(
    {
        "id": ID,
        "revision": ID,
        "edition": ID,
        "pages": {"type": "integer", "minimum": 1, "maximum": 100000},
        "nodes": {"type": "array", "items": NODE, "minItems": 1, "maxItems": 10000},
    }
)
SCHEMA = record(
    {
        "encoding": {"const": 1},
        "revision": {"type": "integer", "minimum": 1},
        "policy": record({"edition": ID, "precedence": {"type": "array", "maxItems": 0}}),
        "enabled": {
            "type": "array",
            "items": record({"book": ID, "revision": ID}),
            "maxItems": 64,
        },
        "books": {"type": "array", "items": BOOK, "minItems": 1, "maxItems": 64},
    }
)
Draft202012Validator.check_schema(SCHEMA)
VALIDATOR = Draft202012Validator(SCHEMA)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


class Forest:
    """Book-local ancestry and explicit cross-book references, never inferred rules."""

    def __init__(self, payload):
        errors = list(VALIDATOR.iter_errors(payload))
        if errors:
            raise ValueError(f"Invalid evidence shape at {list(errors[0].path)}")
        self._payload = copy.deepcopy(payload)
        self.fingerprint = digest(payload)
        self._books = {b["id"]: b for b in self._payload["books"]}
        if len(self._books) != len(payload["books"]):
            raise ValueError("Duplicate book identity")
        self._enabled = {p["book"]: p["revision"] for p in payload["enabled"]}
        if len(self._enabled) != len(payload["enabled"]):
            raise ValueError("Duplicate enabled book pin")
        for ident, revision in self._enabled.items():
            book = self._books.get(ident)
            if (
                not book
                or book["revision"] != revision
                or book["edition"] != payload["policy"]["edition"]
            ):
                raise ValueError("Missing, mismatched or cross-edition enabled source")
        self._nodes = {}
        self.deferred = []
        self.events = []
        for book in self._books.values():
            nodes = {n["id"]: n for n in book["nodes"]}
            if len(nodes) != len(book["nodes"]):
                raise ValueError("Duplicate book-local node identity")
            self._nodes[book["id"]] = nodes
            roots = 0
            for n in nodes.values():
                for endpoint in (n["start"], n["end"]):
                    if endpoint is not None and endpoint[0] > book["pages"]:
                        raise ValueError("Source position outside book")
                if n["start"] is not None and n["end"] is not None and n["start"] > n["end"]:
                    raise ValueError("Reversed source span")
                state, parent = n["parent_state"], n["parent"]
                if state == "known":
                    if parent not in nodes:
                        raise ValueError("Resolved parent missing from same book")
                    p = nodes[parent]
                    for endpoint in (n["start"], n["end"]):
                        if endpoint is None:
                            continue
                        if p["start"] is not None and endpoint < p["start"]:
                            raise ValueError("Child precedes parent span")
                        if p["end"] is not None and endpoint > p["end"]:
                            raise ValueError("Child exceeds parent span")
                elif parent is not None:
                    raise ValueError("Root/unresolved parent must be null")
                if state == "root":
                    roots += 1
                if state == "unresolved" or n["start"] is None or n["end"] is None:
                    if not n.get("reason", "").strip():
                        raise ValueError("Incomplete structure needs explicit reason")
                    self.deferred.append((book["id"], n["id"]))
            if roots != 1:
                raise ValueError("Each book requires exactly one explicit root")
            for n in nodes.values():
                seen = set()
                current = n
                while current["parent_state"] == "known":
                    if current["id"] in seen:
                        raise ValueError("Cyclic structural ancestry")
                    seen.add(current["id"])
                    current = nodes[current["parent"]]

    @property
    def payload(self):
        return copy.deepcopy(self._payload)

    @property
    def nodes(self):
        return copy.deepcopy(self._nodes)

    @property
    def books(self):
        return copy.deepcopy(self._books)

    @property
    def enabled(self):
        return dict(self._enabled)

    def lineage(self, book, node):
        path = []
        current = self._node(book, node)
        while True:
            if (
                current["parent_state"] == "unresolved"
                or current["start"] is None
                or current["end"] is None
            ):
                raise ValueError("Incomplete ancestry/span cannot supply complete context")
            path.append(current)
            if current["parent_state"] == "root":
                return copy.deepcopy(list(reversed(path)))
            current = self._node(book, current["parent"])

    def _node(self, book, node):
        try:
            return self._nodes[book][node]
        except KeyError as error:
            raise ValueError("Unknown book-local node") from error

    def _admit(self, book, allowed):
        if book not in allowed:
            raise PermissionError("Book is not currently accessible")
        if book not in self._enabled:
            raise ValueError("Book is not enabled")

    def context(self, book, node, allowed):
        self._admit(book, allowed)
        path = self.lineage(book, node)
        references = []
        for ref in path[-1]["references"]:
            item = {**ref}
            if ref["book"] not in allowed:
                item["status"] = "inaccessible"
            elif ref["book"] not in self._enabled:
                item["status"] = "disabled"
            elif self._enabled[ref["book"]] != ref["revision"]:
                item["status"] = "revision_mismatch"
            else:
                try:
                    target = self.lineage(ref["book"], ref["node"])
                except ValueError:
                    item["status"] = "unresolved"
                else:
                    item.update(
                        status="resolved",
                        text=target[-1]["text"],
                        path=[n["id"] for n in target],
                    )
            references.append(item)
        return {
            "book": book,
            "revision": self._enabled[book],
            "path": [n["id"] for n in path],
            "titles": [n["title"] for n in path],
            "text": path[-1]["text"],
            "references": references,
            "scope": "structural ancestry only; no inferred applicability or precedence",
        }

    def materialize(self, book, node, allowed):
        return {
            "fingerprint": self.fingerprint,
            "book": book,
            "node": node,
            "context": self.context(book, node, allowed),
        }

    def read_materialized(self, artifact, allowed):
        if artifact["fingerprint"] != self.fingerprint:
            raise ValueError("Stale derived evidence context")
        return self.context(artifact["book"], artifact["node"], allowed)

    def revise(self, book, node, changes):
        if not changes or set(changes) - {
            "title",
            "text",
            "parent",
            "parent_state",
            "start",
            "end",
            "references",
            "reason",
        }:
            raise ValueError("Only evidence content/structure may be repaired; identity is stable")
        payload = copy.deepcopy(self.payload)
        target = next(
            n for b in payload["books"] if b["id"] == book for n in b["nodes"] if n["id"] == node
        )
        target.update(changes)
        payload["revision"] += 1
        revised = Forest(payload)
        revised.events = self.events + [
            {
                "book": book,
                "node": node,
                "old": self.fingerprint,
                "new": revised.fingerprint,
                "invalidated": "all derived artifacts, including ancestor/reference consumers",
            }
        ]
        return revised


def assemble(base, batches):
    payload = copy.deepcopy(base)
    if len(batches) > 64:
        raise ValueError("Batch count exceeds prototype limit")
    for batch in batches:
        books = [b for b in payload["books"] if b["id"] == batch["book"]]
        if len(books) != 1 or books[0]["revision"] != batch["revision"]:
            raise ValueError("Batch book/version mismatch")
        book = books[0]
        for n in batch["nodes"]:
            existing = [old for old in book["nodes"] if old["id"] == n["id"]]
            if existing:
                if existing != [n]:
                    raise ValueError("Conflicting batch overlap")
            else:
                book["nodes"].append(copy.deepcopy(n))
    return Forest(payload)
