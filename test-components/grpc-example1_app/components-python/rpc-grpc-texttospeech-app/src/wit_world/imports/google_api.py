from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some
from ..imports import google_protobuf

@dataclass
class CustomHttpPattern:
    kind: Optional[str]
    path: Optional[str]


@dataclass
class HttpRulePattern_Get:
    value: Optional[str]


@dataclass
class HttpRulePattern_Put:
    value: Optional[str]


@dataclass
class HttpRulePattern_Post:
    value: Optional[str]


@dataclass
class HttpRulePattern_Delete:
    value: Optional[str]


@dataclass
class HttpRulePattern_Patch:
    value: Optional[str]


@dataclass
class HttpRulePattern_Custom:
    value: Optional[CustomHttpPattern]


HttpRulePattern = Union[HttpRulePattern_Get, HttpRulePattern_Put, HttpRulePattern_Post, HttpRulePattern_Delete, HttpRulePattern_Patch, HttpRulePattern_Custom]


class LaunchStage(Enum):
    LAUNCH_STAGE_UNSPECIFIED = 0
    UNIMPLEMENTED = 1
    PRELAUNCH = 2
    EARLY_ACCESS = 3
    ALPHA = 4
    BETA = 5
    GA = 6
    DEPRECATED = 7

@dataclass
class JavaSettingsServiceClassNamesEntry:
    key: Optional[str]
    value: Optional[str]

@dataclass
class PythonSettingsExperimentalFeatures:
    rest_async_io_enabled: Optional[bool]
    protobuf_pythonic_types_enabled: Optional[bool]
    unversioned_package_disabled: Optional[bool]

@dataclass
class DotnetSettingsRenamedServicesEntry:
    key: Optional[str]
    value: Optional[str]

@dataclass
class DotnetSettingsRenamedResourcesEntry:
    key: Optional[str]
    value: Optional[str]

@dataclass
class GoSettingsRenamedServicesEntry:
    key: Optional[str]
    value: Optional[str]

@dataclass
class MethodSettingsLongRunning:
    initial_poll_delay: Optional[google_protobuf.Duration]
    poll_delay_multiplier: Optional[float]
    max_poll_delay: Optional[google_protobuf.Duration]
    total_poll_timeout: Optional[google_protobuf.Duration]

@dataclass
class MethodSettings:
    selector: Optional[str]
    long_running: Optional[MethodSettingsLongRunning]
    auto_populated_fields: List[str]

@dataclass
class SelectiveGapicGeneration:
    methods: List[str]
    generate_omitted_as_internal: Optional[bool]

class ClientLibraryOrganization(Enum):
    CLIENT_LIBRARY_ORGANIZATION_UNSPECIFIED = 0
    CLOUD = 1
    ADS = 2
    PHOTOS = 3
    STREET_VIEW = 4
    SHOPPING = 5
    GEO = 6
    GENERATIVE_AI = 7

class ClientLibraryDestination(Enum):
    CLIENT_LIBRARY_DESTINATION_UNSPECIFIED = 0
    GITHUB = 1
    PACKAGE_MANAGER = 2

@dataclass
class CommonLanguageSettings:
    reference_docs_uri: Optional[str]
    destinations: List[ClientLibraryDestination]
    selective_gapic_generation: Optional[SelectiveGapicGeneration]

@dataclass
class JavaSettings:
    library_package: Optional[str]
    service_class_names: List[JavaSettingsServiceClassNamesEntry]
    common: Optional[CommonLanguageSettings]

@dataclass
class CppSettings:
    common: Optional[CommonLanguageSettings]

@dataclass
class PhpSettings:
    common: Optional[CommonLanguageSettings]

@dataclass
class PythonSettings:
    common: Optional[CommonLanguageSettings]
    experimental_features: Optional[PythonSettingsExperimentalFeatures]

@dataclass
class NodeSettings:
    common: Optional[CommonLanguageSettings]

@dataclass
class DotnetSettings:
    common: Optional[CommonLanguageSettings]
    renamed_services: List[DotnetSettingsRenamedServicesEntry]
    renamed_resources: List[DotnetSettingsRenamedResourcesEntry]
    ignored_resources: List[str]
    forced_namespace_aliases: List[str]
    handwritten_signatures: List[str]

@dataclass
class RubySettings:
    common: Optional[CommonLanguageSettings]

@dataclass
class GoSettings:
    common: Optional[CommonLanguageSettings]
    renamed_services: List[GoSettingsRenamedServicesEntry]

@dataclass
class ClientLibrarySettings:
    version: Optional[str]
    launch_stage: Optional[LaunchStage]
    rest_numeric_enums: Optional[bool]
    java_settings: Optional[JavaSettings]
    cpp_settings: Optional[CppSettings]
    php_settings: Optional[PhpSettings]
    python_settings: Optional[PythonSettings]
    node_settings: Optional[NodeSettings]
    dotnet_settings: Optional[DotnetSettings]
    ruby_settings: Optional[RubySettings]
    go_settings: Optional[GoSettings]

@dataclass
class Publishing:
    method_settings: List[MethodSettings]
    new_issue_uri: Optional[str]
    documentation_uri: Optional[str]
    api_short_name: Optional[str]
    github_label: Optional[str]
    codeowner_github_teams: List[str]
    doc_tag_prefix: Optional[str]
    organization: Optional[ClientLibraryOrganization]
    library_settings: List[ClientLibrarySettings]
    proto_reference_documentation_uri: Optional[str]
    rest_reference_documentation_uri: Optional[str]

class FieldBehavior(Enum):
    FIELD_BEHAVIOR_UNSPECIFIED = 0
    OPTIONAL = 1
    REQUIRED = 2
    OUTPUT_ONLY = 3
    INPUT_ONLY = 4
    IMMUTABLE = 5
    UNORDERED_LIST = 6
    NON_EMPTY_DEFAULT = 7
    IDENTIFIER = 8

class ResourceDescriptorHistory(Enum):
    HISTORY_UNSPECIFIED = 0
    ORIGINALLY_SINGLE_PATTERN = 1
    FUTURE_MULTI_PATTERN = 2

class ResourceDescriptorStyle(Enum):
    STYLE_UNSPECIFIED = 0
    DECLARATIVE_FRIENDLY = 1

@dataclass
class ResourceDescriptor:
    type: Optional[str]
    pattern: List[str]
    name_field: Optional[str]
    history: Optional[ResourceDescriptorHistory]
    plural: Optional[str]
    singular: Optional[str]
    style: List[ResourceDescriptorStyle]

@dataclass
class ResourceReference:
    type: Optional[str]
    child_type: Optional[str]

@dataclass
class HttpRule:
    selector: Optional[str]
    get: Optional[str]
    put: Optional[str]
    post: Optional[str]
    delete: Optional[str]
    patch: Optional[str]
    custom: Optional[CustomHttpPattern]
    body: Optional[str]
    response_body: Optional[str]
    additional_bindings: List[Any]

class LazyHttpRule:
    
    def __init__(self, http_rule: HttpRule) -> None:
        raise NotImplementedError

    def get(self) -> HttpRule:
        raise NotImplementedError
    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


@dataclass
class Http:
    rules: List[HttpRule]
    fully_decode_reserved_expansion: Optional[bool]


