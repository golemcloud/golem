from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some


@dataclass
class DescriptorProtoReservedRange:
    start: Optional[int]
    end: Optional[int]

@dataclass
class ExtensionRangeOptionsDeclaration:
    number: Optional[int]
    full_name: Optional[str]
    type: Optional[str]
    reserved: Optional[bool]
    repeated: Optional[bool]

class ExtensionRangeOptionsVerificationState(Enum):
    DECLARATION = 0
    UNVERIFIED = 1

class FieldDescriptorProtoType(Enum):
    TYPE_DOUBLE = 0
    TYPE_FLOAT = 1
    TYPE_INT64 = 2
    TYPE_UINT64 = 3
    TYPE_INT32 = 4
    TYPE_FIXED64 = 5
    TYPE_FIXED32 = 6
    TYPE_BOOL = 7
    TYPE_STRING = 8
    TYPE_GROUP = 9
    TYPE_MESSAGE = 10
    TYPE_BYTES = 11
    TYPE_UINT32 = 12
    TYPE_ENUM = 13
    TYPE_SFIXED32 = 14
    TYPE_SFIXED64 = 15
    TYPE_SINT32 = 16
    TYPE_SINT64 = 17

class FieldDescriptorProtoLabel(Enum):
    LABEL_OPTIONAL = 0
    LABEL_REPEATED = 1
    LABEL_REQUIRED = 2

@dataclass
class EnumDescriptorProtoEnumReservedRange:
    start: Optional[int]
    end: Optional[int]

class FileOptionsOptimizeMode(Enum):
    SPEED = 0
    CODE_SIZE = 1
    LITE_RUNTIME = 2

class FieldOptionsCType(Enum):
    STRING = 0
    CORD = 1
    STRING_PIECE = 2

class FieldOptionsJSType(Enum):
    JS_NORMAL = 0
    JS_STRING = 1
    JS_NUMBER = 2

class FieldOptionsOptionRetention(Enum):
    RETENTION_UNKNOWN = 0
    RETENTION_RUNTIME = 1
    RETENTION_SOURCE = 2

class FieldOptionsOptionTargetType(Enum):
    TARGET_TYPE_UNKNOWN = 0
    TARGET_TYPE_FILE = 1
    TARGET_TYPE_EXTENSION_RANGE = 2
    TARGET_TYPE_MESSAGE = 3
    TARGET_TYPE_FIELD = 4
    TARGET_TYPE_ONEOF = 5
    TARGET_TYPE_ENUM = 6
    TARGET_TYPE_ENUM_ENTRY = 7
    TARGET_TYPE_SERVICE = 8
    TARGET_TYPE_METHOD = 9

class MethodOptionsIdempotencyLevel(Enum):
    IDEMPOTENCY_UNKNOWN = 0
    NO_SIDE_EFFECTS = 1
    IDEMPOTENT = 2

@dataclass
class UninterpretedOptionNamePart:
    name_part: str
    is_extension: bool

@dataclass
class UninterpretedOption:
    name: List[UninterpretedOptionNamePart]
    identifier_value: Optional[str]
    positive_int_value: Optional[int]
    negative_int_value: Optional[int]
    double_value: Optional[float]
    string_value: Optional[bytes]
    aggregate_value: Optional[str]

class FeatureSetFieldPresence(Enum):
    FIELD_PRESENCE_UNKNOWN = 0
    EXPLICIT = 1
    IMPLICIT = 2
    LEGACY_REQUIRED = 3

class FeatureSetEnumType(Enum):
    ENUM_TYPE_UNKNOWN = 0
    OPEN = 1
    CLOSED = 2

class FeatureSetRepeatedFieldEncoding(Enum):
    REPEATED_FIELD_ENCODING_UNKNOWN = 0
    PACKED = 1
    EXPANDED = 2

class FeatureSetUtf8Validation(Enum):
    UTF8_VALIDATION_UNKNOWN = 0
    VERIFY = 1
    NONE = 2

class FeatureSetMessageEncoding(Enum):
    MESSAGE_ENCODING_UNKNOWN = 0
    LENGTH_PREFIXED = 1
    DELIMITED = 2

class FeatureSetJsonFormat(Enum):
    JSON_FORMAT_UNKNOWN = 0
    ALLOW = 1
    LEGACY_BEST_EFFORT = 2

class FeatureSetEnforceNamingStyle(Enum):
    ENFORCE_NAMING_STYLE_UNKNOWN = 0
    STYLE2024 = 1
    STYLE_LEGACY = 2

@dataclass
class FeatureSet:
    field_presence: Optional[FeatureSetFieldPresence]
    enum_type: Optional[FeatureSetEnumType]
    repeated_field_encoding: Optional[FeatureSetRepeatedFieldEncoding]
    utf8_validation: Optional[FeatureSetUtf8Validation]
    message_encoding: Optional[FeatureSetMessageEncoding]
    json_format: Optional[FeatureSetJsonFormat]
    enforce_naming_style: Optional[FeatureSetEnforceNamingStyle]

@dataclass
class ExtensionRangeOptions:
    uninterpreted_option: List[UninterpretedOption]
    declaration: List[ExtensionRangeOptionsDeclaration]
    features: Optional[FeatureSet]
    verification: Optional[ExtensionRangeOptionsVerificationState]

@dataclass
class DescriptorProtoExtensionRange:
    start: Optional[int]
    end: Optional[int]
    options: Optional[ExtensionRangeOptions]

@dataclass
class FileOptions:
    java_package: Optional[str]
    java_outer_classname: Optional[str]
    java_multiple_files: Optional[bool]
    java_generate_equals_and_hash: Optional[bool]
    java_string_check_utf8: Optional[bool]
    optimize_for: Optional[FileOptionsOptimizeMode]
    go_package: Optional[str]
    cc_generic_services: Optional[bool]
    java_generic_services: Optional[bool]
    py_generic_services: Optional[bool]
    deprecated: Optional[bool]
    cc_enable_arenas: Optional[bool]
    objc_class_prefix: Optional[str]
    csharp_namespace: Optional[str]
    swift_prefix: Optional[str]
    php_class_prefix: Optional[str]
    php_namespace: Optional[str]
    php_metadata_namespace: Optional[str]
    ruby_package: Optional[str]
    features: Optional[FeatureSet]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class MessageOptions:
    message_set_wire_format: Optional[bool]
    no_standard_descriptor_accessor: Optional[bool]
    deprecated: Optional[bool]
    map_entry: Optional[bool]
    deprecated_legacy_json_field_conflicts: Optional[bool]
    features: Optional[FeatureSet]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class OneofOptions:
    features: Optional[FeatureSet]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class OneofDescriptorProto:
    name: Optional[str]
    options: Optional[OneofOptions]

@dataclass
class EnumOptions:
    allow_alias: Optional[bool]
    deprecated: Optional[bool]
    deprecated_legacy_json_field_conflicts: Optional[bool]
    features: Optional[FeatureSet]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class ServiceOptions:
    features: Optional[FeatureSet]
    deprecated: Optional[bool]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class MethodOptions:
    deprecated: Optional[bool]
    idempotency_level: Optional[MethodOptionsIdempotencyLevel]
    features: Optional[FeatureSet]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class MethodDescriptorProto:
    name: Optional[str]
    input_type: Optional[str]
    output_type: Optional[str]
    options: Optional[MethodOptions]
    client_streaming: Optional[bool]
    server_streaming: Optional[bool]

@dataclass
class ServiceDescriptorProto:
    name: Optional[str]
    method: List[MethodDescriptorProto]
    options: Optional[ServiceOptions]

@dataclass
class SourceCodeInfoLocation:
    path: List[int]
    span: List[int]
    leading_comments: Optional[str]
    trailing_comments: Optional[str]
    leading_detached_comments: List[str]

@dataclass
class SourceCodeInfo:
    location: List[SourceCodeInfoLocation]

class GeneratedCodeInfoAnnotationSemantic(Enum):
    NONE = 0
    SET = 1
    ALIAS = 2

@dataclass
class GeneratedCodeInfoAnnotation:
    path: List[int]
    source_file: Optional[str]
    begin: Optional[int]
    end: Optional[int]
    semantic: Optional[GeneratedCodeInfoAnnotationSemantic]

@dataclass
class GeneratedCodeInfo:
    annotation: List[GeneratedCodeInfoAnnotation]

class Edition(Enum):
    EDITION_UNKNOWN = 0
    EDITION_LEGACY = 1
    EDITION_PROTO2 = 2
    EDITION_PROTO3 = 3
    EDITION_E2023 = 4
    EDITION_E2024 = 5
    EDITION_E1_TEST_ONLY = 6
    EDITION_E2_TEST_ONLY = 7
    EDITION_E99997_TEST_ONLY = 8
    EDITION_E99998_TEST_ONLY = 9
    EDITION_E99999_TEST_ONLY = 10
    EDITION_MAX = 11

@dataclass
class FieldOptionsEditionDefault:
    edition: Optional[Edition]
    value: Optional[str]

@dataclass
class FieldOptionsFeatureSupport:
    edition_introduced: Optional[Edition]
    edition_deprecated: Optional[Edition]
    deprecation_warning: Optional[str]
    edition_removed: Optional[Edition]

@dataclass
class FieldOptions:
    ctype: Optional[FieldOptionsCType]
    packed: Optional[bool]
    jstype: Optional[FieldOptionsJSType]
    lazy: Optional[bool]
    unverified_lazy: Optional[bool]
    deprecated: Optional[bool]
    weak: Optional[bool]
    debug_redact: Optional[bool]
    retention: Optional[FieldOptionsOptionRetention]
    targets: List[FieldOptionsOptionTargetType]
    edition_defaults: List[FieldOptionsEditionDefault]
    features: Optional[FeatureSet]
    feature_support: Optional[FieldOptionsFeatureSupport]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class FieldDescriptorProto:
    name: Optional[str]
    number: Optional[int]
    label: Optional[FieldDescriptorProtoLabel]
    type: Optional[FieldDescriptorProtoType]
    type_name: Optional[str]
    extendee: Optional[str]
    default_value: Optional[str]
    oneof_index: Optional[int]
    json_name: Optional[str]
    options: Optional[FieldOptions]
    proto3_optional: Optional[bool]

@dataclass
class EnumValueOptions:
    deprecated: Optional[bool]
    features: Optional[FeatureSet]
    debug_redact: Optional[bool]
    feature_support: Optional[FieldOptionsFeatureSupport]
    uninterpreted_option: List[UninterpretedOption]

@dataclass
class EnumValueDescriptorProto:
    name: Optional[str]
    number: Optional[int]
    options: Optional[EnumValueOptions]

@dataclass
class EnumDescriptorProto:
    name: Optional[str]
    value: List[EnumValueDescriptorProto]
    options: Optional[EnumOptions]
    reserved_range: List[EnumDescriptorProtoEnumReservedRange]
    reserved_name: List[str]

@dataclass
class FeatureSetDefaultsFeatureSetEditionDefault:
    edition: Optional[Edition]
    overridable_features: Optional[FeatureSet]
    fixed_features: Optional[FeatureSet]

@dataclass
class FeatureSetDefaults:
    defaults: List[FeatureSetDefaultsFeatureSetEditionDefault]
    minimum_edition: Optional[Edition]
    maximum_edition: Optional[Edition]

@dataclass
class Duration:
    seconds: Optional[int]
    nanos: Optional[int]

@dataclass
class DescriptorProto:
    name: Optional[str]
    field: List[FieldDescriptorProto]
    extension: List[FieldDescriptorProto]
    nested_type: List[Any]
    enum_type: List[EnumDescriptorProto]
    extension_range: List[DescriptorProtoExtensionRange]
    oneof_decl: List[OneofDescriptorProto]
    options: Optional[MessageOptions]
    reserved_range: List[DescriptorProtoReservedRange]
    reserved_name: List[str]

class LazyDescriptorProto:
    
    def __init__(self, descriptor_proto: DescriptorProto) -> None:
        raise NotImplementedError

    def get(self) -> DescriptorProto:
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
class FileDescriptorProto:
    name: Optional[str]
    package: Optional[str]
    dependency: List[str]
    public_dependency: List[int]
    weak_dependency: List[int]
    message_type: List[DescriptorProto]
    enum_type: List[EnumDescriptorProto]
    service: List[ServiceDescriptorProto]
    extension: List[FieldDescriptorProto]
    options: Optional[FileOptions]
    source_code_info: Optional[SourceCodeInfo]
    syntax: Optional[str]
    edition: Optional[Edition]

@dataclass
class FileDescriptorSet:
    file: List[FileDescriptorProto]


