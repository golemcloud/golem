#[cfg(test)]
mod tests {
    use golem_common::model::component_metadata::ComponentMetadata;

    // This tests that serializers are consistent across PRs.
    // If this tests fails, then we have made a backwards incompatible change to the serialization spec.
    // Metadata JSON gets written to DB as serialized string in component repo.
    #[test]
    fn test_metadata_spec_serde() {
        fn test_serde(json: &str) {
            let result: ComponentMetadata = serde_json::from_str(json).unwrap();
            let json2 = serde_json::to_value(result.clone()).unwrap();
            // println!("{}", json2);
            let result2: ComponentMetadata = serde_json::from_value(json2.clone()).unwrap();

            assert_eq!(result, result2);
        }

        test_serde(
            r#"
        {
          "exports": [
            {
              "Instance": {
                "name": "golem:it/api",
                "functions": [
                  {
                    "name": "initialize-cart",
                    "results": [],
                    "parameters": [
                      {
                        "tpe": {
                          "Str": {}
                        },
                        "name": "user-id"
                      }
                    ]
                  },
                  {
                    "name": "add-item",
                    "results": [],
                    "parameters": [
                      {
                        "tpe": {
                          "Record": [
                            [
                              "product-id",
                              {
                                "Str": {}
                              }
                            ],
                            [
                              "name",
                              {
                                "Str": {}
                              }
                            ],
                            [
                              "price",
                              {
                                "F32": {}
                              }
                            ],
                            [
                              "quantity",
                              {
                                "U32": {}
                              }
                            ]
                          ]
                        },
                        "name": "item"
                      }
                    ]
                  },
                  {
                    "name": "remove-item",
                    "results": [],
                    "parameters": [
                      {
                        "tpe": {
                          "Str": {}
                        },
                        "name": "product-id"
                      }
                    ]
                  },
                  {
                    "name": "update-item-quantity",
                    "results": [],
                    "parameters": [
                      {
                        "tpe": {
                          "Str": {}
                        },
                        "name": "product-id"
                      },
                      {
                        "tpe": {
                          "U32": {}
                        },
                        "name": "quantity"
                      }
                    ]
                  },
                  {
                    "name": "checkout",
                    "results": [
                      {
                        "tpe": {
                          "Variant": [
                            [
                              "error",
                              {
                                "Str": {}
                              }
                            ],
                            [
                              "success",
                              {
                                "Record": [
                                  [
                                    "order-id",
                                    {
                                      "Str": {}
                                    }
                                  ]
                                ]
                              }
                            ]
                          ]
                        }
                      }
                    ],
                    "parameters": []
                  },
                  {
                    "name": "get-cart-contents",
                    "results": [
                      {
                        "tpe": {
                          "List": {
                            "Record": [
                              [
                                "product-id",
                                {
                                  "Str": {}
                                }
                              ],
                              [
                                "name",
                                {
                                  "Str": {}
                                }
                              ],
                              [
                                "price",
                                {
                                  "F32": {}
                                }
                              ],
                              [
                                "quantity",
                                {
                                  "U32": {}
                                }
                              ]
                            ]
                          }
                        }
                      }
                    ],
                    "parameters": []
                  }
                ]
              }
            }
          ],
          "producers": [
            {
              "fields": [
                {
                  "name": "processed-by",
                  "values": [
                    {
                      "name": "wit-component",
                      "version": "0.14.0"
                    },
                    {
                      "name": "cargo-component",
                      "version": "0.1.0 (e57d1d1 2023-08-31 wasi:134dddc)"
                    }
                  ]
                }
              ]
            },
            {
              "fields": [
                {
                  "name": "language",
                  "values": [
                    {
                      "name": "Rust",
                      "version": ""
                    }
                  ]
                },
                {
                  "name": "processed-by",
                  "values": [
                    {
                      "name": "rustc",
                      "version": "1.72.1 (d5c2e9c34 2023-09-13)"
                    },
                    {
                      "name": "clang",
                      "version": "15.0.6"
                    },
                    {
                      "name": "wit-component",
                      "version": "0.14.0"
                    },
                    {
                      "name": "wit-bindgen-rust",
                      "version": "0.11.0"
                    }
                  ]
                }
              ]
            },
            {
              "fields": [
                {
                  "name": "language",
                  "values": [
                    {
                      "name": "Rust",
                      "version": ""
                    }
                  ]
                },
                {
                  "name": "processed-by",
                  "values": [
                    {
                      "name": "rustc",
                      "version": "1.72.0 (5680fa18f 2023-08-23)"
                    }
                  ]
                }
              ]
            },
            {
              "fields": [
                {
                  "name": "processed-by",
                  "values": [
                    {
                      "name": "wit-component",
                      "version": "0.14.0"
                    }
                  ]
                }
              ]
            },
            {
              "fields": [
                {
                  "name": "processed-by",
                  "values": [
                    {
                      "name": "wit-component",
                      "version": "0.14.0"
                    }
                  ]
                }
              ]
            }
          ]
        }
        "#,
        );

        test_serde(
            r#"
          {
           "exports":[
              {
                 "Instance":{
                    "name":"golem:it/api",
                    "functions":[
                       {
                          "name":"sleep",
                          "results":[
                             {
                                "tpe":{
                                   "Result": [
                                      null,
                                      {
                                          "Str":{
                                          }
                                      }
                                   ]
                                },
                                "name":null
                             }
                          ],
                          "parameters":[
                             {
                                "tpe":{
                                   "U64":{

                                   }
                                },
                                "name":"secs"
                             }
                          ]
                       }
                    ]
                 }
              }
           ],
           "producers":[
              {
                 "fields":[
                    {
                       "name":"processed-by",
                       "values":[
                          {
                             "name":"wit-component",
                             "version":"0.14.0"
                          },
                          {
                             "name":"cargo-component",
                             "version":"0.1.0 (e57d1d1 2023-08-31 wasi:134dddc)"
                          }
                       ]
                    }
                 ]
              },
              {
                 "fields":[
                    {
                       "name":"language",
                       "values":[
                          {
                             "name":"Rust",
                             "version":""
                          }
                       ]
                    },
                    {
                       "name":"processed-by",
                       "values":[
                          {
                             "name":"rustc",
                             "version":"1.72.1 (d5c2e9c34 2023-09-13)"
                          },
                          {
                             "name":"clang",
                             "version":"15.0.6"
                          },
                          {
                             "name":"wit-component",
                             "version":"0.14.0"
                          },
                          {
                             "name":"wit-bindgen-rust",
                             "version":"0.11.0"
                          }
                       ]
                    }
                 ]
              },
              {
                 "fields":[
                    {
                       "name":"language",
                       "values":[
                          {
                             "name":"Rust",
                             "version":""
                          }
                       ]
                    },
                    {
                       "name":"processed-by",
                       "values":[
                          {
                             "name":"rustc",
                             "version":"1.72.0 (5680fa18f 2023-08-23)"
                          }
                       ]
                    }
                 ]
              },
              {
                 "fields":[
                    {
                       "name":"processed-by",
                       "values":[
                          {
                             "name":"wit-component",
                             "version":"0.14.0"
                          }
                       ]
                    }
                 ]
              },
              {
                 "fields":[
                    {
                       "name":"processed-by",
                       "values":[
                          {
                             "name":"wit-component",
                             "version":"0.14.0"
                          }
                       ]
                    }
                 ]
              }
           ]
        }
          "#,
        );
    }
}
