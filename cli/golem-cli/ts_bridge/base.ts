export type PhantomId = string;

export type GolemServer =
    | { type: 'local' }
    | { type: 'cloud'; token: string }
    | { type: 'custom'; url: string; token: string };

export type AroundInvokeHook = {
    beforeInvoke: (request: AgentInvocationRequest) => Promise<void>;
    afterInvoke: (request: AgentInvocationRequest, result: JsonResult<AgentInvocationResult, any>) => Promise<void>;
};

export type Configuration = {
    server: GolemServer,
    application: ApplicationName,
    environment: EnvironmentName,
    aroundInvokeHook?: AroundInvokeHook,
}

export type ApplicationName = string;
export type EnvironmentName = string;
export type AgentTypeName = string;
export type IdempotencyKey = string;

export type UntypedDataValue =
    | { type: "Tuple"; elements: UntypedElementValue[] }
    | { type: "Multimodal"; elements: UntypedNamedElementValue[] };

export type UntypedElementValue =
    | { type: "ComponentModel"; value: unknown }
    | { type: "UnstructuredText"; value: TextReference }
    | { type: "UnstructuredBinary"; value: BinaryReference };

export interface UntypedNamedElementValue {
    name: string;
    value: UntypedElementValue;
}

export type Url = {
    value: string
}

export type TextSource = {
    data: string; textType?: TextType
}

export type TextReference =
    | { type: "Url"; value: string }
    | { type: "Inline"; data: string; textType?: TextType };

export const TextReference = {
    fromUnstructuredText<LC extends LanguageCode[]>(input: UnstructuredText<LC>): TextReference {
        if (input.tag === 'url') {
            return {
                type: 'Url',
                value: input.val,
            };
        } else {
            return {
                type: 'Inline',
                    data: input.val,
                    textType: input.languageCode
                        ? {languageCode: input.languageCode as string}
                        : undefined,
            };
        }
    }
}

export interface TextType {
    languageCode: string;
}

export type BinarySource = {
    data: Uint8Array; binaryType: BinaryType
}

export type BinaryReference =
    | { type: "Url"; value: string }
    | { type: "Inline"; data: Uint8Array; binaryType: BinaryType };

export const BinaryReference = {
    fromUnstructuredBinary<MT extends MimeType[] | MimeType>(input: UnstructuredBinary<MT>): BinaryReference {
        if (input.tag === 'url') {
            return {
                type: 'Url',
                value: input.val,
            };
        } else {
            return {
                type: 'Inline',
                    data: input.val,
                binaryType: {mimeType: input.mimeType as string},
            };
        }
    }
}

export interface BinaryType {
    mimeType: string;
}

export type DataValue = UntypedDataValue;

export type AgentInvocationMode = "await" | "schedule";

export interface AgentInvocationRequest {
    appName: ApplicationName;
    envName: EnvironmentName;
    agentTypeName: AgentTypeName;
    parameters: DataValue;
    phantomId?: PhantomId;
    methodName: string;
    methodParameters: DataValue;
    mode: AgentInvocationMode;
    scheduleAt?: string; // ISO 8601 datetime
    idempotencyKey?: IdempotencyKey;
}

export interface AgentInvocationResult {
    result?: DataValue;
}

export async function invokeAgent(
    server: GolemServer,
    request: AgentInvocationRequest,
    aroundInvokeHook?: AroundInvokeHook,
): Promise<AgentInvocationResult> {
    let baseUrl: string;
    let token: string;

    switch (server.type) {
        case 'local':
            baseUrl = "http://localhost:9881";
            token = "__REPLACE_WITH_LOCAL_TOKEN__";
            break;
        case 'cloud':
            baseUrl = "https://api.golem.cloud";
            token = server.token;
            break;
        case 'custom':
            baseUrl = server.url;
            token = server.token;
            break;
    }

    const headers: HeadersInit = {
        "Content-Type": "application/json",
    };

    if (token) {
        headers["Authorization"] = `Bearer ${token}`;
    }

    if (request.idempotencyKey) {
        headers["Idempotency-Key"] = request.idempotencyKey!;
    }

    if (aroundInvokeHook) {
        await aroundInvokeHook.beforeInvoke(request);
    }

    try {
      const rawResponse = await fetch(
          `${baseUrl}/v1/agents/invoke-agent`,
          {
            method: "POST",
            headers,
            body: JSON.stringify(request),
          },
      );

      if (!rawResponse.ok) {
          const body = await rawResponse.text().catch(() => undefined);
          if (body) {
            throw new Error(`Agent invocation failed: ${rawResponse.statusText}, ${body}`);
          } else {
            throw new Error(`Agent invocation failed: ${rawResponse.statusText}`);
          }
      }

      let response = await (rawResponse.json() as Promise<AgentInvocationResult>);

      if (aroundInvokeHook) {
          await aroundInvokeHook.afterInvoke(request, {ok: response});
      }

      return response;
    } catch (e) {
        await aroundInvokeHook?.afterInvoke(request, {err: e});
        throw e;
    }
}

/// The Result type representation in Golem's JSON type mapping
export type JsonResult<Ok, Err> = { ok: Ok, err?: undefined } | { ok?: undefined, err: Err };

export type RemoteMethod<Args extends any[], R> = {
    (...args: Args): Promise<R>;
    trigger: (...args: Args) => void;
    schedule: (scheduleAt: string, ...args: Args) => void;
};

export function createRemoteMethod<Args extends any[], R>(
    getServer: () => GolemServer,
    getRequest: () => AgentInvocationRequest,
    encode: (args: Args) => DataValue,
    decode: (result: AgentInvocationResult) => R
): RemoteMethod<Args, R> {
    const result = async function (...args: Args): Promise<R> {
        const invokeResult = await invokeAgent(getServer(), {
            ...getRequest(),
            methodParameters: encode(args),
            mode: "await",
            scheduleAt: undefined
        })
        return decode(invokeResult);
    }
    result.trigger = function (...args: Args): void {
        const _ = invokeAgent(getServer(), {
            ...getRequest(),
            methodParameters: encode(args),
            mode: "schedule",
            scheduleAt: undefined,
        });
    }
    result.schedule = function (scheduleAt: string, ...args: Args): void {
        const _ = invokeAgent(getServer(), {
            ...getRequest(),
            methodParameters: encode(args),
            mode: "schedule",
            scheduleAt,
        });
    }
    return result;
}

type LanguageCode = string;

/**
 * Represents unstructured text input, which can be either a URL or inline text.
 *
 * Example usage:
 *
 * ```ts
 *
 * function foo(input: UnstructuredText) {..}
 *
 * // With language codes
 * function bar(input: UnstructuredText<['en', 'de']>) {..}
 *
 *
 * foo(UnstructuredText.fromInline("hello"));
 * foo(UnstructuredText.fromUrl("http://.."'));
 *
 * bar(UnstructuredText.fromInline("hello", 'en')); // with language code
 *
 * ```
 */
export type UnstructuredText<LC extends LanguageCode[] = []> =
    | {
    tag: 'url';
    val: string;
}
    | {
    tag: 'inline';
    val: string;
    languageCode?: LC[number];
};

export const UnstructuredText = {
    fromUntypedElementValue<LC extends string[] = []>(
        parameterName: string,
        elementValue: UntypedElementValue,
        allowedCodes: string[],
    ): UnstructuredText<LC> {
        if (elementValue.type === 'UnstructuredText') {
            return UnstructuredText.fromDataValue<LC>(
                parameterName,
                elementValue.value,
                allowedCodes,
            );
        } else {
            throw new Error(`Invalid element value type for parameter ${parameterName}. Expected 'unstructuredText', got '${elementValue.type}'`);
        }
    },

    fromDataValue<LC extends string[] = []>(
        parameterName: string,
        dataValue: TextReference,
        allowedCodes: string[],
    ): UnstructuredText<LC> {
        if (dataValue.type === 'Url') {
            return {
                tag: 'url',
                val: dataValue.value,
            };
        } else {
            if (allowedCodes.length > 0) {
                if (!dataValue.textType) {
                    throw new Error(`Language code is required. Allowed codes: ${allowedCodes.join(', ')}`);
                }

                if (!allowedCodes.includes(dataValue.textType.languageCode)) {
                    throw new Error(
                        `Invalid value for parameter ${parameterName}. Language code \`${dataValue.textType.languageCode}\` is not allowed. Allowed codes: ${allowedCodes.join(', ')}`,
                    );
                }

                return {
                    tag: 'inline',
                    val: dataValue.data,
                    languageCode: dataValue.textType.languageCode,
                };
            } else {
                return {
                    tag: 'inline',
                    val: dataValue.data,
                };
            }
        }
    },

    /**
     * Creates `UnstructuredText` from a URL.
     *
     * ```ts
     * function foo(input: UnstructuredText) {..}
     *
     * foo(UnstructuredText.fromUrl("hello"));
     * ```
     *
     * @param urlValue A URL string
     *
     */
    fromUrl(urlValue: string): UnstructuredText {
        return {
            tag: 'url',
            val: urlValue,
        };
    },

    /**
     * Creates `UnstructuredText` from inline text data.
     *
     * ```ts
     * function foo(input: UnstructuredText<['en', 'de']>) {..}
     *
     * foo(UnstructuredText.fromInline("hello", 'en'));
     * ```
     *
     * If defining separately, please annotate the types to infer the types.
     *
     * ```ts
     *
     * const x: UnstructuredText<['en', 'de']> = UnstructuredText.fromInline("hello", 'en');
     *
     * foo(x);
     *
     * ```
     *
     * @param data
     * @param languageCode - The language code
     * @returns A `TextInput` object with `languageCode` set to `'en'`.
     */
    fromInline<LC extends LanguageCode[] = []>(
        data: string,
        languageCode?: LC[number],
    ): UnstructuredText<LC> {
        return {
            tag: 'inline',
            val: data,
            languageCode: languageCode,
        };
    },
};


/**
 * Represents unstructured binary input, which can be either a URL or inline binary data.
 *
 * Example usage:
 *
 * ```ts
 * const inlineBinary: UnstructuredBinary<'application/json'> =
 *   UnstructuredBinary.fromInline(Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream");
 *
 * const urlBinary: UnstructuredBinary =
 *   UnstructuredBinary.fromUrl("https://example.com/file.bin");
 *```
 *
 * If no mime types are specified, any mime type is allowed. Note that
 * when using `inline` you always need to pass a mime-type as we don't allow
 * unstructured-binary without mime type.
 *
 * ```ts
 *  function foo(input: UnstructuredBinary) {..} // any mime type allowed
 *  function bar(input: UnstructuredBinary<['application/json', 'image/png']>) {..} // only application/json and image/png allowed
 *
 *  const imageBinary: UnstructuredBinary =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "image/jpeg");
 *
 *  const textBinary: UnstructuredBinary<'text/plain'> =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "text/plain");
 *
 *  foo(imageBinary); // allowed
 *  foo(textBinary); // allowed
 *
 *  bar(imageBinary); // not allowed
 *
 *  const appJsonBinary: UnstructuredBinary<'application/json'> =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "application/json");
 *
 *  bar(appJsonBinary); // allowed
 *
 * ```
 */
type MimeType = string;

export type UnstructuredBinary<MT extends MimeType[] | MimeType = MimeType> =
    | {
    tag: 'url';
    val: string;
}
    | {
    tag: 'inline';
    val: Uint8Array;
    mimeType: MT extends MimeType[] ? MT[number] : MimeType;
};

export const UnstructuredBinary = {
    fromUntypedElementValue<MT extends string[] | MimeType = MimeType>(parameterName: string, elementValue: UntypedElementValue, allowedMimeTypes: string[]): UnstructuredBinary<MT> {
        if (elementValue.type === 'UnstructuredBinary') {
            return UnstructuredBinary.fromDataValue<MT>(
                parameterName,
                elementValue.value,
                allowedMimeTypes,
            );
        } else {
            throw new Error(`Invalid element value type for parameter ${parameterName}. Expected 'unstructuredBinary', got '${elementValue.type}'`);
        }
    },

    fromDataValue<MT extends string[] | MimeType = MimeType>(
        parameterName: string,
        dataValue: BinaryReference,
        allowedMimeTypes: string[],
    ): UnstructuredBinary<MT> {
        if (dataValue.type === 'Url') {
            return {
                tag: 'url',
                val: dataValue.value,
            } as UnstructuredBinary<MT>;
        } else {
            if (
                allowedMimeTypes.length > 0 &&
                !allowedMimeTypes.includes(dataValue.binaryType.mimeType)
            ) {
                throw new Error(
                    `Invalid value for parameter ${parameterName}. Mime type \`${dataValue.binaryType.mimeType}\` is not allowed. Allowed mime types: ${allowedMimeTypes.join(', ')}`,
                );
            } else {
                return {
                    tag: 'inline',
                    val: dataValue.data,
                    mimeType: dataValue.binaryType.mimeType,
                } as UnstructuredBinary<MT>;
            }
        }
    },

    /**
     *
     * Creates a `UnstructuredBinary` from a URL.
     *
     * Example usage:
     *
     * ```ts
     *
     * const urlBinary: UnstructuredBinary =
     *   UnstructuredBinary.fromUrl("https://example.com/file.bin");
     *
     * ```
     *
     * @param urlValue
     */
    fromUrl(urlValue: string): UnstructuredBinary {
        return {
            tag: 'url',
            val: urlValue,
        };
    },

    /**
     * Creates a `UnstructuredBinary` from inline binary data.
     *
     * Example usage:
     *
     * ```ts
     *
     * const inlineBinary: UnstructuredBinary<'application/json'> =
     *   UnstructuredBinary.fromInline(Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream");
     *
     * ```
     *
     * @param data
     * @param mimeType
     */
    fromInline<MT extends MimeType[] | MimeType = MimeType>(
        data: Uint8Array,
        mimeType: MT extends MimeType[] ? MT[number] : MimeType,
    ): UnstructuredBinary<MT> {
        return {
            tag: 'inline',
            val: data,
            mimeType: mimeType,
        };
    },
};


export function encodeOption<T>(value: T | undefined, encode: (v: T) => unknown): unknown {
    if (value === undefined || value === null) {
        return null;
    } else {
        return encode(value);
    }
}

export function decodeOption<T>(value: unknown | undefined | null, decode: (v: unknown) => T): T | undefined {
    if (value === undefined || value === null) {
        return undefined;
    } else {
        return decode(value);
    }
}


