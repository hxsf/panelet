// Local modification: Panelet's inherited NOTE4C experiment fixes ItemBuf's
// owned Serde round trip. This file differs from upstream Rune 0.14.2.
// See ../../LOCAL-PATCH.md for the patch scope and upstream revision.

use core::fmt;
use core::marker::PhantomData;

use serde::de::{self, Error as _};
use serde::ser::{self, SerializeSeq};

use crate::alloc::alloc::Allocator;
use crate::alloc::String;
use crate::item::{Component, Item, ItemBuf};

// This mirrors `ComponentRef`'s default externally tagged Serde form, but
// owns string data. Binary decoders such as Ciborium cannot provide the
// borrowed `&str` expected by `ComponentRef<'de>`.
#[derive(serde::Deserialize)]
enum OwnedComponentRef {
    Crate(String),
    Str(String),
    Id(usize),
}

impl OwnedComponentRef {
    fn into_component(self) -> crate::alloc::Result<Component> {
        Ok(match self {
            Self::Crate(value) => Component::Crate(value.try_into_boxed_str()?),
            Self::Str(value) => Component::Str(value.try_into_boxed_str()?),
            Self::Id(value) => Component::Id(value),
        })
    }
}

impl ser::Serialize for Item {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;

        for item in self.iter() {
            seq.serialize_element(&item)?;
        }

        seq.end()
    }
}

impl<A: Allocator> ser::Serialize for ItemBuf<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_ref().serialize(serializer)
    }
}

impl<'de, A: Allocator> de::Deserialize<'de> for ItemBuf<A>
where
    A: Default,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_bytes(BytesVisitor(PhantomData))
    }
}

struct BytesVisitor<A>(PhantomData<A>);

impl<'de, A: Allocator> de::Visitor<'de> for BytesVisitor<A>
where
    A: Default,
{
    type Value = ItemBuf<A>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "item buffer deserialization to be implemented")
    }

    fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
    where
        S: de::SeqAccess<'de>,
    {
        let mut buf = ItemBuf::new_in(A::default());

        // `Item::serialize` emits the externally tagged `ComponentRef` form.
        // Decode its owned equivalent so non-borrowing binary decoders work.
        while let Some(component) = seq.next_element::<OwnedComponentRef>()? {
            let component = component.into_component().map_err(S::Error::custom)?;
            buf.push(component).map_err(S::Error::custom)?;
        }

        Ok(buf)
    }
}
