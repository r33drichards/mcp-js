# Sessions and Heaps

In stateful mode, heap state is persisted by content hash rather than by a
mutable session object. A completed execution can return a heap snapshot key,
and passing that key back into a later call resumes from that V8 state.

Session history is related but separate. Session logging tracks named sessions
and execution history, while heap tags provide a separate metadata layer for
searching and organizing heap snapshots.
