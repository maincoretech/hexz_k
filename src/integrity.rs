//! Publisher-authenticated integrity layered on the standard Hexz 0.8 format.
//!
//! Hexz's native Ed25519 signature authenticates the master index and metadata.
//! This module stores hashes for the header, paged indices, dictionary, and
//! physical data blocks inside metadata, so the native signature transitively
//! covers every semantically meaningful archive region. Ordinary Hexz readers
//! ignore the extra JSON field and remain compatible.

use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, VerifyingKey};
use hexz_core::format::header::Header;
use hexz_core::format::index::{IndexPage, MasterIndex, PageEntry};
use hexz_core::store::StorageBackend;
use hexz_store::local::MmapBackend;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PROFILE_FIELD: &str = "hexz_k_integrity";
const PROFILE_NAME: &str = "hexz-k-integrity-v1";
const ZERO_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Runtime policy for publisher integrity verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntegrityPolicy {
    /// Open archives without signature or integrity verification.
    #[default]
    Disabled,
    /// Verify signed archives, while still accepting unsigned archives.
    VerifyIfSigned([u8; 32]),
    /// Reject archives without a valid signature and complete integrity profile.
    Required([u8; 32]),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntegrityManifest {
    profile: String,
    header_blake3: String,
    main_pages: Vec<HashedRegion>,
    auxiliary_pages: Vec<HashedRegion>,
    blocks: Vec<HashedRegion>,
    dictionary: Option<HashedRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HashedRegion {
    offset: u64,
    length: u32,
    blake3: String,
}

#[derive(Debug)]
struct VerifiedBlock {
    digest: [u8; 32],
    verified: AtomicBool,
}

/// An immutable mmap backend that authenticates each physical data block on
/// its first cache miss. The atomic fast path avoids hashing it again.
#[derive(Debug)]
struct IntegrityBackend {
    inner: MmapBackend,
    blocks: HashMap<(u64, usize), VerifiedBlock>,
}

impl StorageBackend for IntegrityBackend {
    fn read_exact(&self, offset: u64, len: usize) -> hexz_common::Result<bytes::Bytes> {
        let bytes = self.inner.read_exact(offset, len)?;
        if let Some(expected) = self.blocks.get(&(offset, len))
            && !expected.verified.load(Ordering::Acquire)
        {
            let actual = blake3::hash(&bytes);
            if actual.as_bytes() != &expected.digest {
                return Err(hexz_common::Error::Format(format!(
                    "archive data block failed publisher integrity verification at {offset}"
                )));
            }
            expected.verified.store(true, Ordering::Release);
        }
        Ok(bytes)
    }

    fn len(&self) -> u64 {
        self.inner.len()
    }
}

/// Generate a raw Ed25519 keypair using Hexz's native key format.
pub fn generate_keypair(private_key: &Path, public_key: &Path) -> Result<()> {
    hexz_common::sign::generate_keypair(private_key, public_key)
        .context("failed to generate Hexz signing keypair")
}

/// Add the integrity profile to a freshly packed archive and sign it with the
/// standard Hexz Ed25519 signature mechanism.
pub fn seal_archive(archive: &Path, private_key: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(archive)
        .with_context(|| format!("failed to open archive for signing: {}", archive.display()))?;
    let mut header = Header::read_from(&mut file)?;
    if header.signature_offset.is_some() || header.signature_length.is_some() {
        bail!("archive is already signed");
    }
    if !header.parent_paths.is_empty() {
        bail!("integrity profile currently requires a self-contained archive");
    }

    let master = MasterIndex::read_from(&mut file, header.index_offset)?;
    let main_pages = hash_pages(&mut file, &master.main_pages)?;
    let auxiliary_pages = hash_pages(&mut file, &master.auxiliary_pages)?;
    let blocks = hash_stored_blocks(
        &mut file,
        master.main_pages.iter().chain(&master.auxiliary_pages),
    )?;
    let dictionary = match (header.dictionary_offset, header.dictionary_length) {
        (Some(offset), Some(length)) => Some(hash_region(&mut file, offset, length)?),
        (None, None) => None,
        _ => bail!("archive has incomplete dictionary metadata"),
    };

    let metadata = read_metadata_value(&mut file, &header)?;
    let mut manifest = IntegrityManifest {
        profile: PROFILE_NAME.to_owned(),
        header_blake3: ZERO_DIGEST.to_owned(),
        main_pages,
        auxiliary_pages,
        blocks,
        dictionary,
    };
    let metadata_offset = file.seek(SeekFrom::End(0))?;
    let placeholder = with_integrity_manifest(metadata.clone(), &manifest)?;
    let placeholder_bytes = serde_json::to_vec(&placeholder)?;
    header.metadata_offset = Some(metadata_offset);
    header.metadata_length =
        Some(u32::try_from(placeholder_bytes.len()).context("integrity metadata is too large")?);
    manifest.header_blake3 = hex_digest(&canonical_header_digest(&header)?);
    let metadata = with_integrity_manifest(metadata, &manifest)?;
    let metadata_bytes = serde_json::to_vec(&metadata)?;
    if metadata_bytes.len() != placeholder_bytes.len() {
        bail!("canonical integrity metadata length changed unexpectedly");
    }

    file.write_all(&metadata_bytes)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bincode::serialize(&header)?)?;
    file.sync_all()?;
    drop(file);

    hexz_ops::sign::sign_archive(archive, private_key)
        .context("failed to apply native Hexz signature")?;
    Ok(())
}

/// Verify a signed archive and return a backend that lazily authenticates data
/// blocks. Page indices and the optional compression dictionary are verified at
/// open because they are small and govern all later random reads.
pub(crate) fn verified_backend(
    path: &Path,
    policy: IntegrityPolicy,
) -> Result<Arc<dyn StorageBackend>> {
    let inner = MmapBackend::new(path)?;
    let header = Header::read_from_backend(&inner)?;
    let public_key = match (policy, header.signature_offset) {
        (IntegrityPolicy::Disabled, _) => return Ok(Arc::new(inner)),
        (IntegrityPolicy::VerifyIfSigned(_), None) => return Ok(Arc::new(inner)),
        (IntegrityPolicy::Required(_), None) => bail!("archive is not signed"),
        (IntegrityPolicy::VerifyIfSigned(key), Some(_))
        | (IntegrityPolicy::Required(key), Some(_)) => key,
    };

    verify_native_signature(&inner, &header, public_key)?;
    let manifest = read_integrity_manifest(&inner, &header)?;
    verify_header(&header, &manifest)?;
    let master = MasterIndex::read_from_backend(&inner, header.index_offset)?;
    let expected_blocks = verify_pages(&inner, &master, &manifest)?;
    verify_dictionary(&inner, &header, manifest.dictionary.as_ref())?;
    let blocks = build_block_map(expected_blocks, &manifest.blocks)?;

    Ok(Arc::new(IntegrityBackend { inner, blocks }))
}

fn verify_native_signature(
    backend: &dyn StorageBackend,
    header: &Header,
    public_key: [u8; 32],
) -> Result<()> {
    let (Some(signature_offset), Some(signature_length)) =
        (header.signature_offset, header.signature_length)
    else {
        bail!("archive signature metadata is incomplete");
    };
    if signature_length != 64 || signature_offset < header.index_offset {
        bail!("archive signature metadata is invalid");
    }
    let signature_end = signature_offset
        .checked_add(u64::from(signature_length))
        .context("archive signature range overflow")?;
    if signature_end != backend.len() {
        bail!("archive contains unsigned trailing data");
    }

    let signed_len = usize::try_from(signature_offset - header.index_offset)
        .context("signed archive region is too large for this platform")?;
    let signed = backend.read_exact(header.index_offset, signed_len)?;
    let digest = Sha256::digest(&signed);
    let raw_signature = backend.read_exact(signature_offset, 64)?;
    let signature_bytes: [u8; 64] = raw_signature
        .as_ref()
        .try_into()
        .expect("signature length was checked");
    let key = VerifyingKey::from_bytes(&public_key).context("invalid Ed25519 public key")?;
    key.verify_strict(&digest, &Signature::from_bytes(&signature_bytes))
        .context("Hexz publisher signature verification failed")
}

fn read_integrity_manifest(
    backend: &dyn StorageBackend,
    header: &Header,
) -> Result<IntegrityManifest> {
    let (Some(offset), Some(length)) = (header.metadata_offset, header.metadata_length) else {
        bail!("signed archive has no metadata");
    };
    let bytes = backend.read_exact(offset, length as usize)?;
    let root: Value = serde_json::from_slice(&bytes).context("invalid archive metadata JSON")?;
    let value = root
        .get(PROFILE_FIELD)
        .cloned()
        .context("signed archive lacks the hexz_k integrity profile")?;
    let manifest: IntegrityManifest = serde_json::from_value(value)?;
    if manifest.profile != PROFILE_NAME {
        bail!("unsupported integrity profile: {}", manifest.profile);
    }
    Ok(manifest)
}

fn verify_header(header: &Header, manifest: &IntegrityManifest) -> Result<()> {
    let expected = parse_digest(&manifest.header_blake3)?;
    let actual = canonical_header_digest(header)?;
    if actual != expected {
        bail!("archive header failed publisher integrity verification");
    }
    Ok(())
}

fn verify_pages(
    backend: &dyn StorageBackend,
    master: &MasterIndex,
    manifest: &IntegrityManifest,
) -> Result<BTreeSet<(u64, u32)>> {
    let mut blocks = BTreeSet::new();
    verify_page_stream(
        backend,
        &master.main_pages,
        &manifest.main_pages,
        &mut blocks,
    )?;
    verify_page_stream(
        backend,
        &master.auxiliary_pages,
        &manifest.auxiliary_pages,
        &mut blocks,
    )?;
    Ok(blocks)
}

fn verify_page_stream(
    backend: &dyn StorageBackend,
    entries: &[PageEntry],
    expected: &[HashedRegion],
    blocks: &mut BTreeSet<(u64, u32)>,
) -> Result<()> {
    if entries.len() != expected.len() {
        bail!("archive index page count does not match its integrity profile");
    }
    for (entry, region) in entries.iter().zip(expected) {
        if entry.offset != region.offset || entry.length != region.length {
            bail!("archive index page layout failed publisher integrity verification");
        }
        let bytes = backend.read_exact(entry.offset, entry.length as usize)?;
        verify_digest(&bytes, &region.blake3, "archive index page")?;
        let page: IndexPage = bincode::deserialize(&bytes)?;
        blocks.extend(
            page.blocks
                .into_iter()
                .filter(|block| !block.is_sparse() && !block.is_parent_ref())
                .map(|block| (block.offset, block.length)),
        );
    }
    Ok(())
}

fn verify_dictionary(
    backend: &dyn StorageBackend,
    header: &Header,
    expected: Option<&HashedRegion>,
) -> Result<()> {
    match (header.dictionary_offset, header.dictionary_length, expected) {
        (None, None, None) => Ok(()),
        (Some(offset), Some(length), Some(region))
            if offset == region.offset && length == region.length =>
        {
            let bytes = backend.read_exact(offset, length as usize)?;
            verify_digest(&bytes, &region.blake3, "archive compression dictionary")
        }
        _ => bail!("archive dictionary layout does not match its integrity profile"),
    }
}

fn build_block_map(
    expected: BTreeSet<(u64, u32)>,
    regions: &[HashedRegion],
) -> Result<HashMap<(u64, usize), VerifiedBlock>> {
    let described = regions
        .iter()
        .map(|region| (region.offset, region.length))
        .collect::<BTreeSet<_>>();
    if described.len() != regions.len() || described != expected {
        bail!("archive data block layout does not match its integrity profile");
    }
    regions
        .iter()
        .map(|region| {
            Ok((
                (region.offset, region.length as usize),
                VerifiedBlock {
                    digest: parse_digest(&region.blake3)?,
                    verified: AtomicBool::new(false),
                },
            ))
        })
        .collect()
}

fn hash_pages(file: &mut File, entries: &[PageEntry]) -> Result<Vec<HashedRegion>> {
    entries
        .iter()
        .map(|entry| hash_region(file, entry.offset, entry.length))
        .collect()
}

fn hash_stored_blocks<'a>(
    file: &mut File,
    pages: impl Iterator<Item = &'a PageEntry>,
) -> Result<Vec<HashedRegion>> {
    let mut blocks = BTreeSet::new();
    for entry in pages {
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut bytes = vec![0; entry.length as usize];
        file.read_exact(&mut bytes)?;
        let page: IndexPage = bincode::deserialize(&bytes)?;
        blocks.extend(
            page.blocks
                .into_iter()
                .filter(|block| !block.is_sparse() && !block.is_parent_ref())
                .map(|block| (block.offset, block.length)),
        );
    }
    blocks
        .into_iter()
        .map(|(offset, length)| hash_region(file, offset, length))
        .collect()
}

fn hash_region(file: &mut File, offset: u64, length: u32) -> Result<HashedRegion> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    Ok(HashedRegion {
        offset,
        length,
        blake3: hex_digest(blake3::hash(&bytes).as_bytes()),
    })
}

fn read_metadata_value(file: &mut File, header: &Header) -> Result<Value> {
    let (Some(offset), Some(length)) = (header.metadata_offset, header.metadata_length) else {
        bail!("archive has no metadata to extend");
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).context("invalid archive metadata JSON")
}

fn with_integrity_manifest(mut root: Value, manifest: &IntegrityManifest) -> Result<Value> {
    let object = root
        .as_object_mut()
        .context("archive metadata must be a JSON object")?;
    if object.contains_key(PROFILE_FIELD) {
        bail!("archive metadata already contains an integrity profile");
    }
    object.insert(PROFILE_FIELD.to_owned(), serde_json::to_value(manifest)?);
    Ok(root)
}

fn canonical_header_digest(header: &Header) -> Result<[u8; 32]> {
    let mut canonical = header.clone();
    canonical.signature_offset = None;
    canonical.signature_length = None;
    Ok(*blake3::hash(&bincode::serialize(&canonical)?).as_bytes())
}

fn verify_digest(bytes: &[u8], expected: &str, region: &str) -> Result<()> {
    if blake3::hash(bytes).as_bytes() != &parse_digest(expected)? {
        bail!("{region} failed publisher integrity verification");
    }
    Ok(())
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn parse_digest(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        bail!("invalid BLAKE3 digest length in integrity profile");
    }
    let mut output = [0; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Ok(output)
}

fn hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => bail!("invalid hexadecimal digest in integrity profile"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::pack::{PackOptions, pack_directory, pack_signed_directory};
    use crate::{ResourcePack, ResourcePackOptions};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hexz-k-integrity-test-{}-{sequence}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, path: &str) -> std::path::PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn options(input: &Path, output: &Path) -> PackOptions {
        PackOptions {
            input: input.display().to_string(),
            output: output.display().to_string(),
            compression: "zstd".to_owned(),
            encrypt: true,
            block_size: 4096,
            password: Some("test-password".to_owned()),
        }
    }

    fn read_public_key(path: &Path) -> [u8; 32] {
        std::fs::read(path).unwrap().try_into().unwrap()
    }

    fn flip_byte(path: &Path, offset: u64) -> u8 {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        let mut byte = [0];
        file.read_exact(&mut byte).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(&[byte[0] ^ 0x80]).unwrap();
        byte[0]
    }

    fn restore_byte(path: &Path, offset: u64, byte: u8) {
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(&[byte]).unwrap();
    }

    #[test]
    fn signed_profile_is_standard_compatible_and_detects_each_tamper_layer() {
        let temp = TestDir::new();
        let input = temp.join("input");
        std::fs::create_dir_all(&input).unwrap();
        let payload = vec![0x5a; 32 * 1024];
        std::fs::write(input.join("asset.bin"), &payload).unwrap();
        let archive = temp.join("game.hxz");
        let private_key = temp.join("private.key");
        let public_key = temp.join("public.key");
        generate_keypair(&private_key, &public_key).unwrap();
        pack_signed_directory(&options(&input, &archive), &private_key).unwrap();

        // The file remains a standard Hexz 0.8 archive and its native verifier
        // accepts the signature.
        hexz_ops::sign::verify_archive(&archive, &public_key).unwrap();
        assert_eq!(
            ResourcePack::open(&archive, Some("test-password"))
                .unwrap()
                .read_file("asset.bin")
                .unwrap(),
            payload
        );

        let key = read_public_key(&public_key);
        let mut required = ResourcePackOptions::memory_constrained().require_integrity(key);
        required.verify_password_on_open = false;
        let verified =
            ResourcePack::open_with_options(&archive, Some("test-password"), required).unwrap();
        assert_eq!(verified.read_file("asset.bin").unwrap(), payload);

        let mut file = File::open(&archive).unwrap();
        let original_header = Header::read_from(&mut file).unwrap();
        let master = MasterIndex::read_from(&mut file, original_header.index_offset).unwrap();
        let page_entry = master.main_pages.first().unwrap();
        file.seek(SeekFrom::Start(page_entry.offset)).unwrap();
        let mut page_bytes = vec![0; page_entry.length as usize];
        file.read_exact(&mut page_bytes).unwrap();
        let page: IndexPage = bincode::deserialize(&page_bytes).unwrap();
        let data_offset = page
            .blocks
            .iter()
            .find(|block| !block.is_sparse() && !block.is_parent_ref())
            .unwrap()
            .offset;
        drop(file);

        // Data verification is lazy: opening succeeds, but the first affected
        // resource read fails.
        let data_byte = flip_byte(&archive, data_offset);
        let pack =
            ResourcePack::open_with_options(&archive, Some("test-password"), required).unwrap();
        assert!(pack.read_file("asset.bin").is_err());
        restore_byte(&archive, data_offset, data_byte);

        let page_byte = flip_byte(&archive, page_entry.offset);
        assert!(
            ResourcePack::open_with_options(&archive, Some("test-password"), required).is_err()
        );
        restore_byte(&archive, page_entry.offset, page_byte);

        // Native Hexz signatures do not cover the header. The profile does.
        let mut changed_header = original_header.clone();
        changed_header.block_size += 1;
        let mut file = OpenOptions::new().write(true).open(&archive).unwrap();
        file.write_all(&bincode::serialize(&changed_header).unwrap())
            .unwrap();
        drop(file);
        assert!(
            ResourcePack::open_with_options(&archive, Some("test-password"), required).is_err()
        );
    }

    #[test]
    fn optional_policy_accepts_unsigned_but_required_policy_rejects_it() {
        let temp = TestDir::new();
        let input = temp.join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("asset.bin"), b"ordinary hexz").unwrap();
        let archive = temp.join("plain.hxz");
        pack_directory(&options(&input, &archive)).unwrap();
        let key = [7; 32];

        let optional = ResourcePackOptions {
            integrity: IntegrityPolicy::VerifyIfSigned(key),
            ..ResourcePackOptions::memory_constrained()
        };
        assert!(ResourcePack::open_with_options(&archive, Some("test-password"), optional).is_ok());
        assert!(
            ResourcePack::open_with_options(
                &archive,
                Some("test-password"),
                ResourcePackOptions::memory_constrained().require_integrity(key),
            )
            .is_err()
        );
    }
}
