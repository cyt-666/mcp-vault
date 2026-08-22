use std::{
    io,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug};
use mcp_vault_storage_fs::{
    ContentHash, DestinationPolicy, DurabilityPolicy, HistoryStore, StorageError, StorageOptions,
    VaultStorage, install_staged_directory, rollback_directory_swaps,
};
use tempfile::tempdir;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::sync::mpsc;

fn path(value: &str) -> VaultPath {
    VaultPath::parse(value).unwrap()
}

fn options() -> StorageOptions {
    StorageOptions {
        durability: DurabilityPolicy::None,
        minimum_free_bytes: 0,
        ..StorageOptions::default()
    }
}

fn context(root: PathBuf) -> VaultContext {
    VaultContext::new(
        VaultId::new(),
        VaultSlug::new("default").unwrap(),
        root,
        Revision::ZERO,
    )
    .unwrap()
}

#[tokio::test]
async fn concurrent_writes_create_shared_missing_parents_idempotently() {
    const WRITE_COUNT: usize = 32;

    let directory = tempdir().unwrap();
    let storage = VaultStorage::new(
        &context(directory.path().join("content")),
        VaultPathPolicy::default(),
        options(),
    );
    storage.ensure_root().await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(WRITE_COUNT));
    let mut tasks = Vec::with_capacity(WRITE_COUNT);
    for index in 0..WRITE_COUNT {
        let storage = storage.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            storage
                .create_dir_all(&path("shared/missing/parents"))
                .await?;
            storage
                .write_bytes(
                    &path(&format!("shared/missing/parents/file-{index}.md")),
                    format!("payload-{index}").as_bytes(),
                    DestinationPolicy::MustNotExist,
                )
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
}

struct ChunkedReader {
    bytes: Vec<u8>,
    offset: usize,
    chunk_size: usize,
}

impl ChunkedReader {
    fn new(bytes: Vec<u8>, chunk_size: usize) -> Self {
        Self {
            bytes,
            offset: 0,
            chunk_size,
        }
    }
}

impl AsyncRead for ChunkedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.offset == self.bytes.len() {
            return Poll::Ready(Ok(()));
        }
        let count = self
            .chunk_size
            .min(self.bytes.len() - self.offset)
            .min(buffer.remaining());
        buffer.put_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Poll::Ready(Ok(()))
    }
}

struct FailingReader {
    bytes: Vec<u8>,
    offset: usize,
    failed: bool,
}

impl FailingReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            offset: 0,
            failed: false,
        }
    }
}

impl AsyncRead for FailingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.failed && self.offset < self.bytes.len() {
            let count = (self.bytes.len() - self.offset).min(buffer.remaining());
            buffer.put_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            self.failed = true;
            return Poll::Ready(Ok(()));
        }
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "injected source failure",
        )))
    }
}

#[tokio::test]
async fn streams_large_payload_and_returns_identity_metadata() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("content");
    let context = context(root);
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());

    storage.create_dir_all(&path("notes")).await.unwrap();
    let bytes: Vec<u8> = (0..(2 * 1024 * 1024))
        .map(|index| (index % 251) as u8)
        .collect();
    let receipt = storage
        .write_atomic(
            &path("notes/large.bin"),
            ChunkedReader::new(bytes.clone(), 137),
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap();

    assert_eq!(receipt.size, bytes.len() as u64);
    assert_eq!(receipt.metadata.size, bytes.len() as u64);
    assert_eq!(receipt.metadata.path, Some(path("notes/large.bin")));
    #[cfg(unix)]
    assert!(receipt.metadata.identity.is_some());

    let mut reader = storage.open_read(&path("notes/large.bin")).await.unwrap();
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).await.unwrap();
    assert_eq!(actual, bytes);
    assert_eq!(
        storage.hash_file(&path("notes/large.bin")).await.unwrap().1,
        receipt.content_hash
    );
}

#[tokio::test]
async fn incremental_atomic_chunks_finalize_with_the_same_hash() {
    let directory = tempdir().unwrap();
    let context = context(directory.path().join("content"));
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());
    storage.create_dir_all(&path("notes")).await.unwrap();

    let mut atomic = storage
        .begin_atomic_write(&path("notes/chunked.bin"), DestinationPolicy::MustNotExist)
        .await
        .unwrap();
    atomic.write_chunk(b"first-").await.unwrap();
    atomic.write_chunk(b"second").await.unwrap();
    let progress = atomic.finish().unwrap();
    atomic.sync().await.unwrap();
    let receipt = atomic.commit().await.unwrap();

    assert_eq!(receipt.size, 12);
    assert_eq!(receipt.content_hash, progress.content_hash);
    let mut reader = storage.open_read(&path("notes/chunked.bin")).await.unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"first-second");
}

#[tokio::test]
async fn direct_directory_listing_is_safe_and_deterministic() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("content");
    let context = context(root.clone());
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());
    storage.create_dir_all(&path("notes")).await.unwrap();
    storage
        .write_bytes(&path("notes/a.md"), b"a", DestinationPolicy::MustNotExist)
        .await
        .unwrap();
    storage
        .write_bytes(&path("notes/b.md"), b"b", DestinationPolicy::MustNotExist)
        .await
        .unwrap();
    std::fs::create_dir_all(root.join("_mcp-vault")).unwrap();
    std::fs::write(root.join("_mcp-vault/hidden.md"), b"hidden").unwrap();

    let entries = storage.list_directory(&VaultPath::root()).await.unwrap();
    let paths = entries
        .into_iter()
        .map(|entry| entry.path.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec![path("notes")]);
    let nested = storage.list_directory(&path("notes")).await.unwrap();
    assert_eq!(
        nested
            .into_iter()
            .map(|entry| entry.path.unwrap())
            .collect::<Vec<_>>(),
        vec![path("notes/a.md"), path("notes/b.md")]
    );
}

#[tokio::test]
async fn failed_stream_keeps_previous_file_and_cleans_temp() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("content");
    let context = context(root.clone());
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());

    storage.create_dir_all(&path("notes")).await.unwrap();
    storage
        .write_bytes(
            &path("notes/old.md"),
            b"old complete content",
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap();

    let result = storage
        .write_atomic(
            &path("notes/old.md"),
            FailingReader::new(b"new incomplete content".to_vec()),
            DestinationPolicy::ReplaceExisting,
        )
        .await;
    assert!(matches!(result, Err(StorageError::Io { .. })));

    let mut reader = storage.open_read(&path("notes/old.md")).await.unwrap();
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).await.unwrap();
    assert_eq!(actual, b"old complete content");

    let temporary_count = std::fs::read_dir(root.join("notes"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".mcp-vault-tmp-")
        })
        .count();
    assert_eq!(temporary_count, 0);
}

#[tokio::test]
async fn copy_move_delete_and_reserved_paths_are_safe() {
    let directory = tempdir().unwrap();
    let context = context(directory.path().join("content"));
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());

    storage.create_dir_all(&path("notes")).await.unwrap();
    storage.create_dir_all(&path("archive")).await.unwrap();
    storage
        .write_bytes(
            &path("notes/source.md"),
            b"copy me",
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap();
    storage
        .copy_file(
            &path("notes/source.md"),
            &path("notes/copy.md"),
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap();
    storage
        .move_entry(
            &path("notes/copy.md"),
            &path("archive/moved.md"),
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap();
    storage.delete(&path("archive/moved.md")).await.unwrap();
    storage.create_dir_all(&path("empty")).await.unwrap();
    storage.delete(&path("empty")).await.unwrap();

    let error = storage.stat(&path("_mcp-vault/memory")).await.unwrap_err();
    assert!(matches!(error, StorageError::Domain(_)));
}

#[tokio::test]
async fn bounded_walk_skips_reserved_and_unsafe_entries_without_following_them() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("content");
    let context = context(root.clone());
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());

    storage.create_dir_all(&path("notes/nested")).await.unwrap();
    storage
        .write_bytes(
            &path("notes/nested/today.md"),
            b"today",
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap();
    std::fs::create_dir_all(root.join("_mcp-vault/memory")).unwrap();
    std::fs::write(root.join("_mcp-vault/memory/managed.md"), b"managed").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("notes"), root.join("unsafe-link")).unwrap();

    let (sender, mut receiver) = mpsc::channel(2);
    let task = tokio::spawn(async move { storage.walk_entries(sender).await });
    let mut paths = Vec::new();
    while let Some(metadata) = receiver.recv().await {
        paths.push(metadata.path.unwrap());
    }
    let summary = task.await.unwrap().unwrap();

    assert!(paths.contains(&path("notes")));
    assert!(paths.contains(&path("notes/nested")));
    assert!(paths.contains(&path("notes/nested/today.md")));
    assert!(
        !paths
            .iter()
            .any(|value| value.starts_with(&path("_mcp-vault")))
    );
    #[cfg(unix)]
    assert!(summary.unsafe_entries_skipped >= 1);
    assert_eq!(summary.files_seen, 1);
}

#[tokio::test]
async fn two_vault_contexts_never_share_content_or_history() {
    let directory = tempdir().unwrap();
    let first_context = context(directory.path().join("first"));
    let second_context = context(directory.path().join("second"));
    let first = VaultStorage::with_defaults(&first_context);
    let second = VaultStorage::with_defaults(&second_context);

    first
        .write_bytes(&path("same.md"), b"first", DestinationPolicy::MustNotExist)
        .await
        .unwrap();
    second
        .write_bytes(&path("same.md"), b"second", DestinationPolicy::MustNotExist)
        .await
        .unwrap();

    let mut first_reader = first.open_read(&path("same.md")).await.unwrap();
    let mut first_bytes = Vec::new();
    first_reader.read_to_end(&mut first_bytes).await.unwrap();
    let mut second_reader = second.open_read(&path("same.md")).await.unwrap();
    let mut second_bytes = Vec::new();
    second_reader.read_to_end(&mut second_bytes).await.unwrap();
    assert_eq!(first_bytes, b"first");
    assert_eq!(second_bytes, b"second");

    let history_root = directory.path().join("history");
    let first_history = HistoryStore::new(&first_context, &history_root, options()).unwrap();
    let second_history = HistoryStore::new(&second_context, &history_root, options()).unwrap();
    let first_blob = first_history.put_bytes(b"same history").await.unwrap();
    let second_blob = second_history.put_bytes(b"same history").await.unwrap();
    assert_eq!(first_blob.content_hash, second_blob.content_hash);
    assert!(first_blob.created);
    assert!(second_blob.created);
    assert!(
        history_root
            .join(first_context.id().to_string())
            .join("blobs")
            .join(first_blob.content_hash.to_string().get(..2).unwrap())
            .join(first_blob.content_hash.to_string())
            .is_file()
    );
    assert!(
        history_root
            .join(second_context.id().to_string())
            .join("blobs")
            .join(second_blob.content_hash.to_string().get(..2).unwrap())
            .join(second_blob.content_hash.to_string())
            .is_file()
    );
}

#[tokio::test]
async fn history_deduplicates_and_streams_blobs() {
    let directory = tempdir().unwrap();
    let context = context(directory.path().join("content"));
    let history = HistoryStore::new(&context, directory.path().join("history"), options()).unwrap();

    let first = history.put_bytes(b"durable history").await.unwrap();
    let second = history.put_bytes(b"durable history").await.unwrap();
    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.content_hash, second.content_hash);
    assert!(history.contains(first.content_hash).await.unwrap());

    let mut reader = history.open(first.content_hash).await.unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"durable history");
    assert_eq!(reader.metadata().path, None);
    assert_eq!(
        ContentHash::from_hex(&first.content_hash.to_string()).unwrap(),
        first.content_hash
    );
    assert!(
        !history
            .contains(ContentHash::from_bytes([0_u8; 32]))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn low_disk_headroom_rejects_before_creating_a_temp_file() {
    let directory = tempdir().unwrap();
    let context = context(directory.path().join("content"));
    let storage = VaultStorage::new(
        &context,
        VaultPathPolicy::default(),
        StorageOptions {
            minimum_free_bytes: u64::MAX,
            durability: DurabilityPolicy::None,
            ..StorageOptions::default()
        },
    );
    storage.ensure_root().await.unwrap();

    let error = storage
        .write_bytes(
            &path("blocked.md"),
            b"must not start",
            DestinationPolicy::MustNotExist,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StorageError::InsufficientDiskSpace { .. }));
    assert!(!directory.path().join("content/blocked.md").exists());
}

#[tokio::test]
async fn directory_swap_rollback_removes_a_new_root_when_no_old_root_existed() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("stage");
    let target = directory.path().join("vault");
    tokio::fs::create_dir_all(&source).await.unwrap();
    tokio::fs::write(source.join("note.md"), b"staged")
        .await
        .unwrap();

    let swap = install_staged_directory(&source, &target, ".rollback")
        .await
        .unwrap();
    assert!(target.join("note.md").exists());
    rollback_directory_swaps(&[swap]).await.unwrap();
    assert!(!target.exists());
    assert!(!source.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlink_escape_and_special_file_entries() {
    use std::{os::unix::fs::symlink, process::Command};

    let directory = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let root = directory.path().join("content");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(outside.path().join("secret.md"), b"outside").unwrap();
    symlink(outside.path(), root.join("escape")).unwrap();
    let fifo = root.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );

    let context = context(root);
    let storage = VaultStorage::new(&context, VaultPathPolicy::default(), options());
    let symlink_error = storage.stat(&path("escape/secret.md")).await.unwrap_err();
    assert!(matches!(
        symlink_error,
        StorageError::UnsafeEntry { .. } | StorageError::Io { .. }
    ));
    let special_error = storage.stat(&path("fifo")).await.unwrap_err();
    assert!(matches!(
        special_error,
        StorageError::Domain(_) | StorageError::UnsafeEntry { .. }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_a_symlink_configured_as_the_content_root() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let actual = directory.path().join("actual");
    std::fs::create_dir_all(&actual).unwrap();
    let link = directory.path().join("link");
    symlink(&actual, &link).unwrap();
    let storage = VaultStorage::with_defaults(&context(link));

    assert!(matches!(
        storage.ensure_root().await,
        Err(StorageError::RootSymlink)
    ));
}
