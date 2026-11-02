//! Adapter-owned configuration: reading the keys adapters claim from one source and keeping the
//! sections they return.
use super::{AdapterRegistry, ConfigKeys, SourceAdapter};
use anyhow::Result;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// The registry whose adapters claim keys in the configuration this thread is reading.
    static READ_SCOPE: RefCell<Option<Rc<AdapterRegistry>>> = const { RefCell::new(None) };
}

/// Restores the enclosing read's registry when a read ends, by return, error or unwind.
struct RestoreReadScope(Option<Rc<AdapterRegistry>>);

impl Drop for RestoreReadScope {
    /// Puts the enclosing registry back.
    fn drop(&mut self) {
        READ_SCOPE.set(self.0.take());
    }
}

/// Runs `read` with `adapters` claiming the adapter keys of every configuration it deserializes.
pub(crate) fn reading_with<T>(adapters: &AdapterRegistry, read: impl FnOnce() -> T) -> T {
    let _restore = RestoreReadScope(READ_SCOPE.replace(Some(Rc::new(adapters.clone()))));
    read()
}

/// The keys one configuration source sets, grouped by the adapter that claims them. Flattened
/// into the core configuration, it takes only claimed keys, so the core still reports every other
/// key as unknown, with its text, position and precedence.
#[derive(Debug, Clone, Default)]
pub(crate) struct AdapterKeys(Vec<(&'static str, ConfigKeys)>);

impl AdapterKeys {
    /// The keys `adapter` claims, read so far.
    fn section(&mut self, adapter: &'static str) -> &mut ConfigKeys {
        let index = match self.0.iter().position(|(name, _)| *name == adapter) {
            Some(index) => index,
            None => {
                self.0.push((adapter, ConfigKeys::new()));
                self.0.len() - 1
            }
        };
        &mut self.0[index].1
    }

    /// Removes the keys `adapter` claims, leaving none when the source set none.
    fn take(&mut self, adapter: &str) -> ConfigKeys {
        self.0
            .iter()
            .position(|(name, _)| *name == adapter)
            .map(|index| self.0.remove(index).1)
            .unwrap_or_default()
    }
}

impl<'de> Deserialize<'de> for AdapterKeys {
    /// Reads the claimed keys of the registry in scope; outside [`reading_with`] it fails, since
    /// no registry decides which keys are claimed.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Release the scope before adapter code runs: an adapter may read configuration itself.
        let Some(adapters) = READ_SCOPE.with_borrow(Clone::clone) else {
            return Err(de::Error::custom(
                "adapter configuration keys read with no adapter registry in scope",
            ));
        };
        deserializer.deserialize_struct(
            "AdapterKeys",
            adapters.config_keys(),
            KeysVisitor(&adapters),
        )
    }
}

/// Routes each claimed key to the adapter that claims it.
struct KeysVisitor<'a>(&'a AdapterRegistry);

impl<'de> Visitor<'de> for KeysVisitor<'_> {
    type Value = AdapterKeys;

    /// Names what the visitor reads.
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("adapter configuration keys")
    }

    /// Reads each key with its adapter's typed reader; a repeated key is a duplicate field.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AdapterKeys, A::Error> {
        let mut keys = AdapterKeys::default();
        let mut seen = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            // fail-closed: as a flattened field only claimed keys arrive; any other is refused.
            let Some((key, adapter)) = self.0.config_key_owner(&key) else {
                return Err(de::Error::unknown_field(&key, self.0.config_keys()));
            };
            if seen.contains(&key) {
                return Err(de::Error::duplicate_field(key));
            }
            seen.push(key);
            let into = keys.section(adapter.name());
            map.next_value_seed(KeySeed { adapter, key, into })?;
        }
        Ok(keys)
    }
}

/// One claimed key's value, read by its adapter.
struct KeySeed<'a> {
    adapter: &'static dyn SourceAdapter,
    key: &'static str,
    into: &'a mut ConfigKeys,
}

impl<'de> DeserializeSeed<'de> for KeySeed<'_> {
    type Value = ();

    /// Hands the value to the adapter's reader, keeping its error text.
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        let mut erased = <dyn erased_serde::Deserializer>::erase(deserializer);
        self.adapter
            .read_config_key(self.key, &mut erased, self.into)
            .map_err(de::Error::custom)
    }
}

/// The section each claiming adapter keeps, in registry order. It serializes as their entries,
/// with no adapter-name wrapper.
#[derive(Debug, Clone, Default)]
pub(crate) struct AdapterSections(Vec<(&'static str, ConfigKeys)>);

impl AdapterSections {
    /// Replaces each claiming adapter's section with the one it returns for the keys one source
    /// set; an adapter whose keys the source left out still applies, with none.
    pub(crate) fn apply(
        &mut self,
        adapters: &AdapterRegistry,
        mut keys: AdapterKeys,
    ) -> Result<()> {
        let mut sections = Vec::new();
        for adapter in adapters.config_claimants() {
            sections.push((
                adapter.name(),
                adapter.apply_config(keys.take(adapter.name()))?,
            ));
        }
        self.0 = sections;
        Ok(())
    }

    /// The section `adapter` keeps, or `None` when it claims no keys.
    pub(crate) fn get(&self, adapter: &str) -> Option<&ConfigKeys> {
        self.0
            .iter()
            .find(|(name, _)| *name == adapter)
            .map(|(_, section)| section)
    }
}

impl Serialize for AdapterSections {
    /// Writes every section's entries, in order.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.0.iter().flat_map(|(_, section)| section) {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests;
