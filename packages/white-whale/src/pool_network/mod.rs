pub mod asset;
pub mod denom;
pub mod factory;
pub mod pair;
pub mod querier;
pub mod router;
pub mod token;

#[cfg(test)]
mod testing;

#[cfg(not(target_arch = "wasm32"))]
pub mod mock_querier;

#[allow(clippy::all)]
mod uints {
    use uint::construct_uint;
    construct_uint! {
        pub struct U256(4);
    }
}

pub use uints::U256;
