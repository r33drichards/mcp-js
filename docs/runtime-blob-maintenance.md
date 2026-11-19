# Local runtime blob maintenance

New artifact and console data is content-addressed in `runtime/v2/`. Overwriting
metadata leaves the previous immutable object available to existing readers.
Collection is explicit; there is no automatic retention timer or HTTP endpoint.

Call the Rust API with **every registry sharing the local blob directory**:

```rust,ignore
ExecutionRegistry::collect_runtime_objects(
    &[&registry_a, &registry_b],
    std::time::Duration::from_secs(3600),
)?;
```

Lower-level users can call `RuntimeStore::collect_runtime_objects(&stores, age)`.
The result counts deleted objects. Only unreachable artifact/console objects at
least this old are deleted. Current references, including deduplicated objects
referenced by another participating database, are retained. Heap/filesystem and
legacy UUID objects are outside this collector.

Each metadata database receives a persistent owner UUID; the directory records
registered owners. Omitting an owner fails before deletion. Removing a database
does not automatically deregister its owner: retain its metadata for collection,
or perform an explicit offline migration into a new root.

On Unix local filesystems, readers and publishers hold shared advisory filesystem
leases. Collection obtains an exclusive lease before scanning metadata and deleting
objects. Active operations cause a retryable busy error. Console page snapshots
and iterators retain their lease until dropped. Artifact reads retain it across
metadata and payload access. Age is an additional retention policy, not the
concurrency protection. Low-level callers staging objects and publishing their
own references must retain `RuntimeTree::read_lease()` across both steps; a copied
`ObjectRef` alone does not pin an object.

All writers must cooperate with this protocol. Do not share a managed directory
with old binaries or modify runtime metadata directly during maintenance.
Unscoped writes mark the root unmanaged. Pre-existing runtime objects without
owner registration and opaque legacy artifact metadata also prevent collection;
they need explicit migration rather than inferred ownership. These checks fail
closed. The collector is unsupported for S3, write-through caches, and platforms
without the local locking implementation; it never falls back to another backend.

New references are stored separately in `runtime_refs:v1:<hex tree name>`.
Artifact index records contain the typed payload reference alongside the header;
console descriptors include length and newline information. Legacy inline data
and explicit DynamoDB envelope references remain readable. Marker-only formats
from earlier unmerged versions of this PR cannot be distinguished from arbitrary
inline payloads, so they are not automatically reinterpreted.
