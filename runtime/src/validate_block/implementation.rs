// Copyright 2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus. If not, see <http://www.gnu.org/licenses/>.

//! The actual implementation of the validate block functionality.

use crate::WitnessData;
use runtime_primitives::traits::{
	Block as BlockT, Header as HeaderT, Hash as HashT
};
use executive::ExecuteBlock;

use substrate_trie::{MemoryDB, read_trie_value, delta_trie_root};

use rstd::{slice, ptr, cmp, vec::Vec, boxed::Box, mem};

use hash_db::HashDB;

use parachain::ValidationParams;

static mut STORAGE: Option<Box<dyn Storage>> = None;
/// The message to use as expect message while accessing the `STORAGE`.
const STORAGE_SET_EXPECT: &str =
	"`STORAGE` needs to be set before calling this function.";
const STORAGE_ROOT_LEN: usize = 32;

/// Extract the hashing algorithm type from the given block type.
type HashingOf<B> = <<B as BlockT>::Header as HeaderT>::Hashing;

/// Abstract the storage into a trait without `Block` generic.
trait Storage {
	/// Retrieve the value for the given key.
	fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

	/// Insert the given key and value.
	fn insert(&mut self, key: &[u8], value: &[u8]);

	/// Remove key and value.
	fn remove(&mut self, key: &[u8]);

	/// Calculate the storage root.
	fn storage_root(&mut self) -> [u8; STORAGE_ROOT_LEN];
}

/// Validate a given parachain block on a validator.
#[doc(hidden)]
pub fn validate_block<B: BlockT, E: ExecuteBlock<B>>(
	params: ValidationParams,
) {
	use codec::Decode;

	let block_data = crate::ParachainBlockData::<B>::decode(&mut &params.block_data[..])
		.expect("Invalid parachain block data");

	let parent_head = B::Header::decode(&mut &params.parent_head[..]).expect("Invalid parent head");

	// TODO: Add `PolkadotInherent`.
	let block = B::new(block_data.header, block_data.extrinsics);
	assert!(parent_head.hash() == *block.header().parent_hash(), "Invalid parent hash");

	let storage = WitnessStorage::<B>::new(
		block_data.witness_data,
		block_data.witness_data_storage_root,
	).expect("Witness data and storage root always match; qed");

	let _guard = unsafe {
		STORAGE = Some(Box::new(storage));
		(
			// Replace storage calls with our own implementations
			rio::ext_get_allocated_storage.replace_implementation(ext_get_allocated_storage),
			rio::ext_get_storage_into.replace_implementation(ext_get_storage_into),
			rio::ext_set_storage.replace_implementation(ext_set_storage),
			rio::ext_exists_storage.replace_implementation(ext_exists_storage),
			rio::ext_clear_storage.replace_implementation(ext_clear_storage),
			rio::ext_storage_root.replace_implementation(ext_storage_root),
		)
	};

	E::execute_block(block);
}

/// The storage implementation used when validating a block that is using the
/// witness data as source.
struct WitnessStorage<B: BlockT> {
	witness_data: MemoryDB<<HashingOf<B> as HashT>::Hasher>,
	overlay: hashbrown::HashMap<Vec<u8>, Option<Vec<u8>>>,
	storage_root: B::Hash,
}

impl<B: BlockT> WitnessStorage<B> {
	/// Initialize from the given witness data and storage root.
	///
	/// Returns an error if given storage root was not found in the witness data.
	fn new(
		data: WitnessData,
		storage_root: B::Hash,
	) -> Result<Self, &'static str> {
		let mut db = MemoryDB::default();
		data.into_iter().for_each(|i| { db.insert(&[], &i); });

		if !db.contains(&storage_root, &[]) {
			return Err("Witness data does not contain given storage root.")
		}

		Ok(Self {
			witness_data: db,
			overlay: Default::default(),
			storage_root,
		})
	}
}

impl<B: BlockT> Storage for WitnessStorage<B> {
	fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
		self.overlay.get(key).cloned().or_else(|| {
			read_trie_value(
				&self.witness_data,
				&self.storage_root,
				key,
			).ok()
		}).unwrap_or(None)
	}

	fn insert(&mut self, key: &[u8], value: &[u8]) {
		self.overlay.insert(key.to_vec(), Some(value.to_vec()));
	}

	fn remove(&mut self, key: &[u8]) {
		self.overlay.insert(key.to_vec(), None);
	}

	fn storage_root(&mut self) -> [u8; STORAGE_ROOT_LEN] {
		let root = match delta_trie_root(
			&mut self.witness_data,
			self.storage_root.clone(),
			self.overlay.drain()
		) {
			Ok(root) => root,
			Err(_) => return [0; STORAGE_ROOT_LEN],
		};

		assert!(root.as_ref().len() <= STORAGE_ROOT_LEN);
		let mut res = [0; STORAGE_ROOT_LEN];
		res.copy_from_slice(root.as_ref());
		res
	}
}

unsafe fn ext_get_allocated_storage(
	key_data: *const u8,
	key_len: u32,
	written_out: *mut u32,
) -> *mut u8 {
	let key = slice::from_raw_parts(key_data, key_len as usize);
	match STORAGE.as_mut().expect(STORAGE_SET_EXPECT).get(key) {
		Some(value) => {
			let mut out_value: Vec<_> = value.clone();
			*written_out = out_value.len() as u32;
			let ptr = out_value.as_mut_ptr();
			mem::forget(out_value);
			ptr
		},
		None => {
			*written_out = u32::max_value();
			ptr::null_mut()
		}
	}
}

unsafe fn ext_set_storage(
	key_data: *const u8,
	key_len: u32,
	value_data: *const u8,
	value_len: u32,
) {
	let key = slice::from_raw_parts(key_data, key_len as usize);
	let value = slice::from_raw_parts(value_data, value_len as usize);

	STORAGE.as_mut().expect(STORAGE_SET_EXPECT).insert(key, value);
}

unsafe fn ext_get_storage_into(
	key_data: *const u8,
	key_len: u32,
	value_data: *mut u8,
	value_len: u32,
	value_offset: u32,
) -> u32 {
	let key = slice::from_raw_parts(key_data, key_len as usize);
	let out_value = slice::from_raw_parts_mut(value_data, value_len as usize);

	match STORAGE.as_mut().expect(STORAGE_SET_EXPECT).get(key) {
		Some(value) => {
			let value = &value[value_offset as usize..];
			let len = cmp::min(value_len as usize, value.len());
			out_value[..len].copy_from_slice(&value[..len]);
			len as u32
		},
		None => {
			u32::max_value()
		}
	}
}

unsafe fn ext_exists_storage(key_data: *const u8, key_len: u32) -> u32 {
	let key = slice::from_raw_parts(key_data, key_len as usize);

	if STORAGE.as_mut().expect(STORAGE_SET_EXPECT).get(key).is_some() {
		1
	} else {
		0
	}
}

unsafe fn ext_clear_storage(key_data: *const u8, key_len: u32) {
	let key = slice::from_raw_parts(key_data, key_len as usize);

	STORAGE.as_mut().expect(STORAGE_SET_EXPECT).remove(key);
}

unsafe fn ext_storage_root(result: *mut u8) {
	let res = STORAGE.as_mut().expect(STORAGE_SET_EXPECT).storage_root();
	let result = slice::from_raw_parts_mut(result, STORAGE_ROOT_LEN);
	result.copy_from_slice(&res);
}