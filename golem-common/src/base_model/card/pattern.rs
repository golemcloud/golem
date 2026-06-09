// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::class::card_permission_classes;
use super::class::*;
use serde::{Deserialize, Serialize};

macro_rules! define_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionPattern {
            $($variant(ClassPermissionPattern<$class>),)+
        }
    };
}

macro_rules! define_permission_target {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionTarget {
            $($variant(ClassPermissionTarget<$class>),)+
        }
    };
}

macro_rules! define_polymorphic_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicPermissionPattern {
            $($variant(PolymorphicClassPermissionPattern<$class>),)+
        }
    };
}

macro_rules! define_polymorphic_manifest_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicManifestPermissionPattern {
            $($variant(PolymorphicManifestClassPermissionPattern<$class>),)+
        }
    };
}

card_permission_classes!(define_permission_pattern);
card_permission_classes!(define_permission_target);
card_permission_classes!(define_polymorphic_permission_pattern);
card_permission_classes!(define_polymorphic_manifest_permission_pattern);

macro_rules! define_class_name_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! class_name_match {
            ($self:expr) => {
                match $self {
                    $(Self::$variant(_) => <$class as PermissionClass>::NAME,)+
                }
            };
        }
    };
}

card_permission_classes!(define_class_name_match);

macro_rules! define_same_variant_subsumes_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_subsumes_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (Self::$variant(a), Self::$variant(b)) => a.subsumes(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_subsumes_match);

macro_rules! define_same_variant_subsumes_target_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_subsumes_target_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (PermissionPattern::$variant(a), PermissionTarget::$variant(b)) => a.subsumes_target(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_subsumes_target_match);

macro_rules! define_same_variant_target_subsumes_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_target_subsumes_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (Self::$variant(a), Self::$variant(b)) => a.subsumes(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_target_subsumes_match);

macro_rules! define_recipient_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! recipient_match {
            ($self:expr) => {
                match $self {
                    $(
                        Self::$variant(pattern) => &pattern.recipient,
                    )+
                }
            };
        }
    };
}

card_permission_classes!(define_recipient_match);

macro_rules! define_permission_pattern_to_target_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! permission_pattern_to_target_match {
            ($self:expr) => {
                match $self {
                    $(
                        PermissionPattern::$variant(pattern) => PermissionTarget::$variant(ClassPermissionTarget::<$class> {
                            verb: pattern.verb,
                            owner: pattern.owner.clone(),
                            resource: pattern.resource.clone(),
                        }),
                    )+
                }
            };
        }
    };
}

card_permission_classes!(define_permission_pattern_to_target_match);

impl PermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        same_variant_subsumes_match!(self, other)
    }

    pub fn subsumes_target(&self, other: &PermissionTarget) -> bool {
        same_variant_subsumes_target_match!(self, other)
    }

    pub fn recipient(&self) -> &crate::model::card::recipient::RecipientPattern {
        recipient_match!(self)
    }

    pub(crate) fn to_target(&self) -> PermissionTarget {
        permission_pattern_to_target_match!(self)
    }
}

impl PermissionTarget {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        same_variant_target_subsumes_match!(self, other)
    }
}

impl PolymorphicPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}

impl PolymorphicManifestPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}
