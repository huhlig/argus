// Copyright 2026 Hans W. Uhlig
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Ground-truth evaluation corpus for Phase 11 code-derived architecture review.

pub mod cyclic_a {
    pub fn step_a(count: u32) -> u32 {
        if count == 0 {
            0
        } else {
            crate::cyclic_b::step_b(count - 1)
        }
    }
}

pub mod cyclic_b {
    pub fn step_b(count: u32) -> u32 {
        if count == 0 {
            0
        } else {
            crate::cyclic_a::step_a(count - 1)
        }
    }
}

pub mod presentation {
    pub fn render_alert(msg: &str) {
        println!("ALERT: {msg}");
    }
}

pub mod layering_violation {
    pub struct LowLevelStorage {
        pub data: Vec<u8>,
    }

    impl LowLevelStorage {
        pub fn persist(&mut self, item: u8) {
            self.data.push(item);
            crate::presentation::render_alert("persisted byte");
        }
    }
}

pub mod leaky_abstraction {
    pub struct NetworkSocketPool {
        pub raw_file_descriptors: Vec<i32>,
        pub internal_kernel_pointer: *mut u8,
    }

    impl NetworkSocketPool {
        #[must_use]
        pub fn new() -> Self {
            Self {
                raw_file_descriptors: Vec::new(),
                internal_kernel_pointer: std::ptr::null_mut(),
            }
        }
    }

    impl Default for NetworkSocketPool {
        fn default() -> Self {
            Self::new()
        }
    }
}

pub mod low_cohesion {
    pub struct GodComponent {
        pub session_token: String,
        pub image_buffer: Vec<u8>,
        pub tax_rate_basis_points: u16,
    }

    impl GodComponent {
        pub fn authenticate(&mut self, token: &str) -> bool {
            self.session_token = token.to_owned();
            true
        }

        pub fn resize_image(&mut self, factor: usize) {
            self.image_buffer.truncate(self.image_buffer.len() / factor.max(1));
        }

        #[must_use]
        pub fn compute_sales_tax(&self, cents: u64) -> u64 {
            (cents * u64::from(self.tax_rate_basis_points)) / 10_000
        }
    }
}

pub mod facade {
    pub struct TransactionManager;

    impl TransactionManager {
        pub fn execute_transaction<F: FnOnce()>(&self, op: F) {
            op();
        }
    }
}

pub mod bypassed_boundary {
    pub fn direct_disk_mutation() {
        let _ = std::fs::write("raw_table.bin", b"uncoordinated_write");
    }
}

pub mod pattern_dissonance {
    pub fn fetch_user_record(id: u64) -> Option<String> {
        if id == 0 {
            panic!("fatal pattern violation: panicking instead of returning Option or Result");
        }
        Some(format!("user_{id}"))
    }
}

// ---------------------------------------------------------------------------
// Clean Controls
// ---------------------------------------------------------------------------

pub mod clean_layered_subsystem {
    pub struct DataStore {
        items: Vec<String>,
    }

    impl DataStore {
        #[must_use]
        pub fn new() -> Self {
            Self { items: Vec::new() }
        }

        pub fn insert(&mut self, item: String) {
            self.items.push(item);
        }
    }

    impl Default for DataStore {
        fn default() -> Self {
            Self::new()
        }
    }

    pub struct BusinessLogic {
        store: DataStore,
    }

    impl BusinessLogic {
        #[must_use]
        pub fn new(store: DataStore) -> Self {
            Self { store }
        }

        pub fn add_entry(&mut self, item: String) {
            self.store.insert(item);
        }
    }
}

pub mod clean_cohesive_module {
    pub struct TokenBucketRateLimiter {
        capacity: u32,
        tokens: u32,
    }

    impl TokenBucketRateLimiter {
        #[must_use]
        pub fn new(capacity: u32) -> Self {
            Self { capacity, tokens: capacity }
        }

        pub fn try_acquire(&mut self) -> bool {
            if self.tokens > 0 {
                self.tokens -= 1;
                true
            } else {
                false
            }
        }
    }
}
