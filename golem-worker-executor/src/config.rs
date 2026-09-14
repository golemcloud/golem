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

#[cfg(test)]
#[test_r::sequential]
mod tests {
    use figment::providers::{Format, Toml};
    use figment::{Figment, Jail};
    use golem_common::SafeDisplay;
    use golem_common::config::env_config_provider;
    use std::time::Duration;
    use test_r::test;

    use crate::services::golem_config::{
        FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, GolemConfig,
        ResourceUsageMeteringConfig, make_config_loader,
    };

    #[test]
    pub fn config_is_loadable() {
        make_config_loader()
            .load()
            .expect("Failed to load base config");
    }

    #[test]
    fn resource_usage_metering_defaults_all_dimensions_disabled() {
        let config = GolemConfig::default();

        assert_eq!(
            config.resource_usage_metering,
            ResourceUsageMeteringConfig {
                compute: false,
                memory: false,
                filesystem: false,
            }
        );
        let displayed = config.resource_usage_metering.to_safe_string();
        assert!(displayed.contains("compute: false"));
        assert!(displayed.contains("memory: false"));
        assert!(displayed.contains("filesystem: false"));
    }

    #[test]
    fn resource_usage_metering_switches_are_independent() {
        for compute in [false, true] {
            for memory in [false, true] {
                for filesystem in [false, true] {
                    let config = ResourceUsageMeteringConfig {
                        compute,
                        memory,
                        filesystem,
                    };
                    assert_eq!(config.compute, compute);
                    assert_eq!(config.memory, memory);
                    assert_eq!(config.filesystem, filesystem);
                    assert_eq!(config.any_byte_time_enabled(), memory || filesystem);
                }
            }
        }

        assert_eq!(
            ResourceUsageMeteringConfig::all_enabled(),
            ResourceUsageMeteringConfig {
                compute: true,
                memory: true,
                filesystem: true,
            }
        );
    }

    #[test]
    fn filesystem_policy_defaults_and_safe_display_remain_stable() {
        let object_policy = FilesystemObjectLimitPolicyConfig::default();
        assert_eq!(object_policy.objects_per_gib(), 32_768);
        assert_eq!(object_policy.minimum_objects(), 8_192);
        assert_eq!(object_policy.maximum_objects(), 131_072);
        let object_display = object_policy.to_safe_string();
        assert!(object_display.contains("objects per GiB: 32768"));
        assert!(object_display.contains("minimum objects: 8192"));
        assert!(object_display.contains("maximum objects: 131072"));

        let pressure = FilesystemPressureConfig::default();
        assert_eq!(pressure.minimum_available_bytes(), 64 * 1024 * 1024);
        assert_eq!(pressure.target_available_bytes(), 128 * 1024 * 1024);
        assert_eq!(pressure.minimum_available_filesystem_objects(), 8_192);
        assert_eq!(pressure.target_available_filesystem_objects(), 16_384);
        assert_eq!(pressure.reclamation_observation_attempts(), 4);
        assert_eq!(
            pressure.reclamation_observation_delay(),
            Duration::from_millis(25)
        );
        let pressure_display = pressure.to_safe_string();
        assert!(pressure_display.contains("minimum available bytes: 67108864"));
        assert!(pressure_display.contains("target available bytes: 134217728"));
        assert!(pressure_display.contains("minimum available filesystem objects: 8192"));
        assert!(pressure_display.contains("target available filesystem objects: 16384"));
        assert!(pressure_display.contains("reclamation observation attempts: 4"));
        assert!(pressure_display.contains("reclamation observation delay: 25ms"));
    }

    #[test]
    fn object_limit_rejects_zero_objects_per_gib() {
        assert_eq!(
            FilesystemObjectLimitPolicyConfig::new(0, 1, 1).unwrap_err(),
            "objects_per_gib must be greater than zero"
        );
    }

    #[test]
    fn object_limit_rejects_zero_minimum_objects() {
        assert_eq!(
            FilesystemObjectLimitPolicyConfig::new(1, 0, 1).unwrap_err(),
            "minimum_objects must be greater than zero"
        );
    }

    #[test]
    fn object_limit_rejects_zero_maximum_objects() {
        assert_eq!(
            FilesystemObjectLimitPolicyConfig::new(1, 1, 0).unwrap_err(),
            "maximum_objects must be greater than zero"
        );
    }

    #[test]
    fn object_limit_rejects_minimum_above_maximum() {
        assert_eq!(
            FilesystemObjectLimitPolicyConfig::new(1, 2, 1).unwrap_err(),
            "minimum_objects must not exceed maximum_objects"
        );
    }

    #[test]
    fn pressure_rejects_non_increasing_byte_watermarks() {
        for minimum in [10, 11] {
            assert_eq!(
                FilesystemPressureConfig::new(minimum, 10, 1, 2, 1, Duration::ZERO).unwrap_err(),
                "minimum_available_bytes must be less than target_available_bytes"
            );
        }
    }

    #[test]
    fn pressure_rejects_non_increasing_object_watermarks() {
        for minimum in [10, 11] {
            assert_eq!(
                FilesystemPressureConfig::new(1, 2, minimum, 10, 1, Duration::ZERO).unwrap_err(),
                "minimum_available_filesystem_objects must be less than target_available_filesystem_objects"
            );
        }
    }

    #[test]
    fn pressure_rejects_zero_reclamation_observation_attempts() {
        assert_eq!(
            FilesystemPressureConfig::new(1, 2, 1, 2, 0, Duration::ZERO).unwrap_err(),
            "reclamation_observation_attempts must be greater than zero"
        );
    }

    #[test]
    fn object_limit_toml_deserialization_rejects_each_invariant_with_field_name() {
        for (toml, field) in [
            (
                "objects_per_gib = 0\nminimum_objects = 1\nmaximum_objects = 1",
                "objects_per_gib",
            ),
            (
                "objects_per_gib = 1\nminimum_objects = 0\nmaximum_objects = 1",
                "minimum_objects",
            ),
            (
                "objects_per_gib = 1\nminimum_objects = 1\nmaximum_objects = 0",
                "maximum_objects",
            ),
            (
                "objects_per_gib = 1\nminimum_objects = 2\nmaximum_objects = 1",
                "minimum_objects",
            ),
        ] {
            let error = Figment::from(Toml::string(toml))
                .extract::<FilesystemObjectLimitPolicyConfig>()
                .unwrap_err();
            assert!(error.to_string().contains(field), "{error}");
        }
    }

    #[test]
    fn pressure_toml_deserialization_rejects_each_invariant_with_field_name() {
        for (minimum_bytes, target_bytes, minimum_objects, target_objects, attempts, field) in [
            (10, 10, 1, 2, 1, "minimum_available_bytes"),
            (1, 2, 10, 10, 1, "minimum_available_filesystem_objects"),
            (1, 2, 1, 2, 0, "reclamation_observation_attempts"),
        ] {
            let toml = format!(
                "minimum_available_bytes = {minimum_bytes}\n\
                 target_available_bytes = {target_bytes}\n\
                 minimum_available_filesystem_objects = {minimum_objects}\n\
                 target_available_filesystem_objects = {target_objects}\n\
                 reclamation_observation_attempts = {attempts}\n\
                 reclamation_observation_delay = \"25ms\""
            );
            let error = Figment::from(Toml::string(&toml))
                .extract::<FilesystemPressureConfig>()
                .unwrap_err();
            assert!(error.to_string().contains(field), "{error}");
        }
    }

    #[test]
    fn full_config_toml_rejects_invalid_filesystem_policy_before_use() {
        let error = make_config_loader()
            .default_figment()
            .merge(Toml::string(
                "[filesystem_storage.filesystem_object_limit_policy]\nobjects_per_gib = 0",
            ))
            .extract::<GolemConfig>()
            .unwrap_err();

        assert!(error.to_string().contains("objects_per_gib"), "{error}");
    }

    #[test]
    fn environment_config_rejects_invalid_filesystem_policies_before_use() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env(
                "GOLEM__FILESYSTEM_STORAGE__FILESYSTEM_OBJECT_LIMIT_POLICY__OBJECTS_PER_GIB",
                "0",
            );
            let object_error = make_config_loader()
                .default_figment()
                .merge(env_config_provider())
                .extract::<GolemConfig>()
                .unwrap_err();
            assert!(
                object_error.to_string().contains("objects_per_gib"),
                "{object_error}"
            );

            jail.set_env(
                "GOLEM__FILESYSTEM_STORAGE__FILESYSTEM_OBJECT_LIMIT_POLICY__OBJECTS_PER_GIB",
                "32768",
            );
            jail.set_env(
                "GOLEM__FILESYSTEM_STORAGE__PRESSURE__RECLAMATION_OBSERVATION_ATTEMPTS",
                "0",
            );
            let pressure_error = make_config_loader()
                .default_figment()
                .merge(env_config_provider())
                .extract::<GolemConfig>()
                .unwrap_err();
            assert!(
                pressure_error
                    .to_string()
                    .contains("reclamation_observation_attempts"),
                "{pressure_error}"
            );
            Ok(())
        });
    }
}
