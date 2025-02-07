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
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use wasm_builder_runner::{build_current_project, WasmBuilderSource};

fn main() {
	build_current_project(
		"wasm_binary.rs",
		WasmBuilderSource::Git {
			repo: "https://github.com/paritytech/substrate",
			rev: "c7fa536d85df5d6a0fc5cdc3f82b6e5a1a2db640",
		}
	);
}