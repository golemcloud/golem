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

//! `golem:network/dns`: record lookups. Each query is one `ReadRemote` call with the lookup
//! grant check of an address lookup and the DNS retry classification. The A and AAAA records
//! of an answer link their addresses to the queried name in the Store's `ResolvedNames` index,
//! live and on replay, like the answers of an address lookup.

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::dns_target;
use crate::durable_host::concurrent::reserve_invocation_id_before_await;
use crate::durable_host::durability::{
    ClassifiedHostError, CustomInvocationContext, HostFailureKind,
};
use crate::durable_host::p3::{durable_worker_ctx, run_deniable_access_classified};
use crate::preview2::golem::network::dns::{
    CaaRecord, DnsError, DnsRecord, Host, HostWithStore, MxRecord, NaptrRecord, RecordData,
    RecordType, SoaRecord, SrvRecord,
};
use crate::services::golem_config::AgentSocketsConfig;
use crate::workerctx::WorkerCtx;
use golem_common::model::RetryContext;
use golem_common::model::oplog::host_functions::GolemNetworkDnsResolve;
use golem_common::model::oplog::types::{
    SerializableDnsError, SerializableDnsRecord, SerializableDnsRecordData,
    SerializableDnsRecordType, SerializableIpAddress,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNetworkDnsResolve, HostResponseNetworkDnsResolve,
};
use hickory_resolver::config::{NameServerConfig, ResolveHosts, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::net::{DnsError as HickoryDnsError, NetError};
use hickory_resolver::proto::op::ResponseCode;
use hickory_resolver::proto::rr::{Name, RData, RecordType as HickoryRecordType};
use hickory_resolver::{Resolver, TokioResolver};
use std::net::IpAddr;
use wasmtime::component::{Accessor, HasSelf};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn resolve(
        store: &Accessor<U, Self>,
        name: String,
        record_type: RecordType,
    ) -> anyhow::Result<Result<Vec<DnsRecord>, DnsError>> {
        resolve::<U, Ctx>(store, name, record_type)
            .await
            .map_err(anyhow::Error::from)
    }
}

async fn resolve<U: Send + 'static, Ctx: WorkerCtx>(
    store: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    name: String,
    record_type: RecordType,
) -> wasmtime::Result<Result<Vec<DnsRecord>, DnsError>> {
    {
        reserve_invocation_id_before_await(store, durable_worker_ctx::<Ctx, U>)?;
        let custom_invocation = store.guest_task_context::<CustomInvocationContext>()?;
        let record_type = SerializableDnsRecordType::from(record_type);
        let targets = dns_target(&name)
            .map(|target| vec![target])
            .map_err(|error| error.to_string());

        let response = run_deniable_access_classified::<_, _, Ctx, GolemNetworkDnsResolve, _, _>(
            store,
            HostRequestNetworkDnsResolve {
                name: name.clone(),
                record_type,
            },
            DurableFunctionType::ReadRemote,
            targets,
            |response| {
                response
                    .result
                    .as_ref()
                    .err()
                    .map(|error| ClassifiedHostError {
                        kind: classify_dns_error(error),
                        message: format!(
                            "DNS {record_type:?} lookup of '{name}' failed: {error:?}"
                        ),
                    })
            },
            RetryContext::dns(&name),
            || async {
                let resolver = store.with(|mut access| {
                    let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                    if !ctx.state.config.agent_sockets.enabled {
                        return Ok(None);
                    }
                    if ctx.state.dns_resolver.is_none() {
                        ctx.state.dns_resolver =
                            Some(build_resolver(&ctx.state.config.agent_sockets)?);
                    }
                    Ok::<_, NetError>(ctx.state.dns_resolver.clone())
                });
                let result = match resolver {
                    Ok(Some(resolver)) => lookup(&resolver, &name, record_type).await,
                    Ok(None) => Err(SerializableDnsError::PermanentResolverFailure),
                    Err(error) => {
                        tracing::warn!(error = %error, "Failed to build the DNS resolver");
                        Err(SerializableDnsError::PermanentResolverFailure)
                    }
                };
                Ok(HostResponseNetworkDnsResolve { result })
            },
        )
        .await?;

        if let Ok(records) = &response.result {
            store.with(|mut access| {
                durable_worker_ctx::<Ctx, U>(access.data_mut()).link_resolved_addresses(
                    custom_invocation.as_deref(),
                    &name,
                    linked_addresses(records),
                )
            })?;
        }

        Ok(response
            .result
            .map(|records| records.into_iter().map(DnsRecord::from).collect())
            .map_err(DnsError::from))
    }
}

/// The addresses of the A and AAAA records of an answer.
pub fn linked_addresses(records: &[SerializableDnsRecord]) -> impl Iterator<Item = IpAddr> + '_ {
    records.iter().filter_map(|record| match &record.data {
        SerializableDnsRecordData::A(address) | SerializableDnsRecordData::Aaaa(address) => {
            Some(IpAddr::from(address.clone()))
        }
        _ => None,
    })
}

/// The upstream servers of the configuration, else the system resolver configuration. The
/// resolver neither reads the hosts file nor caches answers: every guest query is one query to
/// the servers, and the oplog keeps its answer.
fn build_resolver(config: &AgentSocketsConfig) -> Result<TokioResolver, NetError> {
    let mut builder = if config.dns_servers.is_empty() {
        TokioResolver::builder_tokio()?
    } else {
        let name_servers = config
            .dns_servers
            .iter()
            .map(|server| {
                let mut name_server = NameServerConfig::udp_and_tcp(server.ip());
                for connection in &mut name_server.connections {
                    connection.port = server.port();
                }
                name_server
            })
            .collect();
        Resolver::builder_with_config(
            ResolverConfig::from_parts(None, vec![], name_servers),
            TokioRuntimeProvider::default(),
        )
    };
    let options = builder.options_mut();
    options.cache_size = 0;
    options.use_hosts_file = ResolveHosts::Never;
    builder.build()
}

async fn lookup(
    resolver: &TokioResolver,
    name: &str,
    record_type: SerializableDnsRecordType,
) -> Result<Vec<SerializableDnsRecord>, SerializableDnsError> {
    let Ok(query_name) = Name::from_ascii(name) else {
        return Err(SerializableDnsError::InvalidName);
    };
    if name.is_empty() || name.parse::<IpAddr>().is_ok() {
        return Err(SerializableDnsError::InvalidName);
    }
    let hickory_type = hickory_record_type(record_type);
    let answer = resolver
        .lookup(query_name, hickory_type)
        .await
        .map_err(dns_error)?;
    let records: Vec<_> = answer
        .answers()
        .iter()
        .filter(|record| record.record_type() == hickory_type)
        .filter_map(|record| {
            record_data(&record.data).map(|data| SerializableDnsRecord {
                ttl: record.ttl,
                data,
            })
        })
        .collect();
    if records.is_empty() {
        Err(SerializableDnsError::NoData)
    } else {
        Ok(records)
    }
}

fn hickory_record_type(record_type: SerializableDnsRecordType) -> HickoryRecordType {
    match record_type {
        SerializableDnsRecordType::A => HickoryRecordType::A,
        SerializableDnsRecordType::Aaaa => HickoryRecordType::AAAA,
        SerializableDnsRecordType::Cname => HickoryRecordType::CNAME,
        SerializableDnsRecordType::Mx => HickoryRecordType::MX,
        SerializableDnsRecordType::Ns => HickoryRecordType::NS,
        SerializableDnsRecordType::Ptr => HickoryRecordType::PTR,
        SerializableDnsRecordType::Srv => HickoryRecordType::SRV,
        SerializableDnsRecordType::Txt => HickoryRecordType::TXT,
        SerializableDnsRecordType::Caa => HickoryRecordType::CAA,
        SerializableDnsRecordType::Soa => HickoryRecordType::SOA,
        SerializableDnsRecordType::Naptr => HickoryRecordType::NAPTR,
    }
}

fn domain_name(name: &Name) -> String {
    let name = name.to_ascii();
    name.strip_suffix('.').unwrap_or(&name).to_string()
}

fn record_data(data: &RData) -> Option<SerializableDnsRecordData> {
    Some(match data {
        RData::A(a) => SerializableDnsRecordData::A(IpAddr::V4(a.0).into()),
        RData::AAAA(aaaa) => SerializableDnsRecordData::Aaaa(IpAddr::V6(aaaa.0).into()),
        RData::CNAME(cname) => SerializableDnsRecordData::Cname(domain_name(&cname.0)),
        RData::MX(mx) => SerializableDnsRecordData::Mx {
            preference: mx.preference,
            exchange: domain_name(&mx.exchange),
        },
        RData::NS(ns) => SerializableDnsRecordData::Ns(domain_name(&ns.0)),
        RData::PTR(ptr) => SerializableDnsRecordData::Ptr(domain_name(&ptr.0)),
        RData::SRV(srv) => SerializableDnsRecordData::Srv {
            priority: srv.priority,
            weight: srv.weight,
            port: srv.port,
            target: domain_name(&srv.target),
        },
        RData::TXT(txt) => SerializableDnsRecordData::Txt(
            txt.txt_data.iter().map(|chunk| chunk.to_vec()).collect(),
        ),
        RData::CAA(caa) => SerializableDnsRecordData::Caa {
            issuer_critical: caa.issuer_critical,
            tag: caa.tag.clone(),
            value: caa.value.clone(),
        },
        RData::SOA(soa) => SerializableDnsRecordData::Soa {
            mname: domain_name(&soa.mname),
            rname: domain_name(&soa.rname),
            serial: soa.serial,
            refresh: soa.refresh as u32,
            retry: soa.retry as u32,
            expire: soa.expire as u32,
            minimum: soa.minimum,
        },
        RData::NAPTR(naptr) => SerializableDnsRecordData::Naptr {
            order: naptr.order,
            preference: naptr.preference,
            flags: String::from_utf8_lossy(&naptr.flags).into_owned(),
            services: String::from_utf8_lossy(&naptr.services).into_owned(),
            regexp: String::from_utf8_lossy(&naptr.regexp).into_owned(),
            replacement: domain_name(&naptr.replacement),
        },
        _ => return None,
    })
}

fn dns_error(error: NetError) -> SerializableDnsError {
    match error {
        NetError::Dns(HickoryDnsError::NoRecordsFound(no_records)) => {
            if no_records.response_code == ResponseCode::NXDomain {
                SerializableDnsError::NameNotFound
            } else {
                SerializableDnsError::NoData
            }
        }
        NetError::Dns(HickoryDnsError::ResponseCode(ResponseCode::ServFail))
        | NetError::Timeout
        | NetError::Busy
        | NetError::NoConnections
        | NetError::Io(_) => SerializableDnsError::TemporaryResolverFailure,
        _ => SerializableDnsError::PermanentResolverFailure,
    }
}

/// A transient resolver failure retries under the agent's retry policy, like a failed address
/// lookup; a negative answer or a refusal is returned to the guest.
pub(crate) fn classify_dns_error(error: &SerializableDnsError) -> HostFailureKind {
    match error {
        SerializableDnsError::TemporaryResolverFailure => HostFailureKind::Transient,
        SerializableDnsError::AccessDenied
        | SerializableDnsError::InvalidName
        | SerializableDnsError::NameNotFound
        | SerializableDnsError::NoData
        | SerializableDnsError::PermanentResolverFailure => HostFailureKind::Permanent,
    }
}

impl From<RecordType> for SerializableDnsRecordType {
    fn from(value: RecordType) -> Self {
        match value {
            RecordType::A => Self::A,
            RecordType::Aaaa => Self::Aaaa,
            RecordType::Cname => Self::Cname,
            RecordType::Mx => Self::Mx,
            RecordType::Ns => Self::Ns,
            RecordType::Ptr => Self::Ptr,
            RecordType::Srv => Self::Srv,
            RecordType::Txt => Self::Txt,
            RecordType::Caa => Self::Caa,
            RecordType::Soa => Self::Soa,
            RecordType::Naptr => Self::Naptr,
        }
    }
}

impl From<SerializableDnsError> for DnsError {
    fn from(value: SerializableDnsError) -> Self {
        match value {
            SerializableDnsError::AccessDenied => Self::AccessDenied,
            SerializableDnsError::InvalidName => Self::InvalidName,
            SerializableDnsError::NameNotFound => Self::NameNotFound,
            SerializableDnsError::NoData => Self::NoData,
            SerializableDnsError::TemporaryResolverFailure => Self::TemporaryResolverFailure,
            SerializableDnsError::PermanentResolverFailure => Self::PermanentResolverFailure,
        }
    }
}

impl From<SerializableDnsRecord> for DnsRecord {
    fn from(value: SerializableDnsRecord) -> Self {
        let data = match value.data {
            SerializableDnsRecordData::A(address) | SerializableDnsRecordData::Aaaa(address) => {
                match address {
                    SerializableIpAddress::IPv4 {
                        address: [a, b, c, d],
                    } => RecordData::A((a, b, c, d)),
                    SerializableIpAddress::IPv6 {
                        address: [a, b, c, d, e, f, g, h],
                    } => RecordData::Aaaa((a, b, c, d, e, f, g, h)),
                }
            }
            SerializableDnsRecordData::Cname(name) => RecordData::Cname(name),
            SerializableDnsRecordData::Mx {
                preference,
                exchange,
            } => RecordData::Mx(MxRecord {
                preference,
                exchange,
            }),
            SerializableDnsRecordData::Ns(name) => RecordData::Ns(name),
            SerializableDnsRecordData::Ptr(name) => RecordData::Ptr(name),
            SerializableDnsRecordData::Srv {
                priority,
                weight,
                port,
                target,
            } => RecordData::Srv(SrvRecord {
                priority,
                weight,
                port,
                target,
            }),
            SerializableDnsRecordData::Txt(strings) => RecordData::Txt(strings),
            SerializableDnsRecordData::Caa {
                issuer_critical,
                tag,
                value,
            } => RecordData::Caa(CaaRecord {
                issuer_critical,
                tag,
                value,
            }),
            SerializableDnsRecordData::Soa {
                mname,
                rname,
                serial,
                refresh,
                retry,
                expire,
                minimum,
            } => RecordData::Soa(SoaRecord {
                mname,
                rname,
                serial,
                refresh,
                retry,
                expire,
                minimum,
            }),
            SerializableDnsRecordData::Naptr {
                order,
                preference,
                flags,
                services,
                regexp,
                replacement,
            } => RecordData::Naptr(NaptrRecord {
                order,
                preference,
                naptr_flags: flags,
                services,
                regexp,
                replacement,
            }),
        };
        DnsRecord {
            ttl: value.ttl,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::net::NoRecords;
    use hickory_resolver::proto::op::Query;
    use test_r::test;

    fn no_records(response_code: ResponseCode) -> NetError {
        NetError::Dns(HickoryDnsError::NoRecordsFound(NoRecords::new(
            Query::query(Name::from_ascii("db.test.").unwrap(), HickoryRecordType::A),
            response_code,
        )))
    }

    #[test]
    fn negative_answers_are_permanent_and_server_failures_retry() {
        let cases = [
            (
                no_records(ResponseCode::NXDomain),
                SerializableDnsError::NameNotFound,
            ),
            (
                no_records(ResponseCode::NoError),
                SerializableDnsError::NoData,
            ),
            (
                NetError::Dns(HickoryDnsError::ResponseCode(ResponseCode::ServFail)),
                SerializableDnsError::TemporaryResolverFailure,
            ),
            (
                NetError::Timeout,
                SerializableDnsError::TemporaryResolverFailure,
            ),
            (
                NetError::Dns(HickoryDnsError::ResponseCode(ResponseCode::Refused)),
                SerializableDnsError::PermanentResolverFailure,
            ),
        ];
        for (error, expected) in cases {
            let description = error.to_string();
            let mapped = dns_error(error);
            assert_eq!(mapped, expected, "{description}");
            let kind = classify_dns_error(&mapped);
            assert_eq!(
                kind == HostFailureKind::Transient,
                expected == SerializableDnsError::TemporaryResolverFailure,
                "{description}"
            );
        }
    }
}
