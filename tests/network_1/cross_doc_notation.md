---
bid = "10000000-0000-0000-0000-000000000020"
schema = "Document"
title = "Cross Doc Notation"
---

# Cross Doc Notation

This document links to [tokens] to create a cross-document Epistemic edge.
That link causes `push_relation` to query `session_bb` for the tokens document
node neighbourhood when parsing this file. If parsed before `cross_doc_tokens.md`,
that neighbourhood will later carry a stale cross-document Section edge that could
corrupt the tokens PathMap during the tokens parse.

## String Table Productions

This section references the tokens document.

[tokens]: cross_doc_tokens.md