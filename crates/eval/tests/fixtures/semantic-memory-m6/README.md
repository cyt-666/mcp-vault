# M6 synthetic evaluation corpus

This directory is a frozen, synthetic-only corpus for the semantic-card M6
live-runner boundary. It contains 30 Markdown sources and 60 tasks in the
existing `semantic-memory-manifest-v1` fixture format.

The first six project groups (`p01`–`p06`, 18 sources and 36 tasks) are the
development split. The last four groups (`p07`–`p10`, 12 sources and 24 tasks)
are the holdout split. No source is referenced by both splits. Every task
contains a source fence matching each source's fixed File ID, revision,
SourceRevision ID, and SHA-256 hash.

The corpus covers scope, conditions, exceptions, ordered steps, status,
conflicts/difficult negatives, no-answer behavior, duplicate sources, and
controlled cross-source synthesis. Its evaluation identity is explicitly
`synthetic=true` (`dataset_class=synthetic`, `contains_real_user_data=false`);
this data is not user material and does not establish Provider or live-model
success. The manifest remains mock/replay-only until an explicitly authorized
isolated live run supplies a non-synthetic source snapshot.
