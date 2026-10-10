//! Cross-checking emitted address schemes against declared capabilities.

use ptox::{Capabilities, FileResult};

/// Address schemes emitted by `results` that the provider did not declare.
///
/// A provider that declares no `schemes` (or does not declare capabilities at
/// all) is universal, so there is nothing to mismatch and the returned list is
/// empty. Empty schemes on an address are ignored here: those are a shape error
/// reported by the `results_well_formed` check, and reporting them twice would
/// bury the real signal.
pub(crate) fn undeclared_schemes(
    capabilities: &Capabilities,
    results: &[FileResult],
) -> Vec<String> {
    if capabilities.schemes.is_empty() {
        return Vec::new();
    }

    let mut undeclared = Vec::new();
    for result in results {
        for address in &result.addresses {
            if address.scheme.is_empty()
                || capabilities.allows_scheme(&address.scheme)
                || undeclared.iter().any(|scheme| scheme == &address.scheme)
            {
                continue;
            }
            undeclared.push(address.scheme.clone());
        }
    }
    undeclared
}
