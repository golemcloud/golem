mod bindings;

use crate::bindings::exports::golem::it::api::Guest;
use crate::bindings::golem::api::host::*;

struct Component;

impl Guest for Component {

    fn get_idle_workers(
        template_id: TemplateId
    ) -> Vec<WorkerMetadata> {
        println!(
            "Get idle workers of template: {template_id:?}"
        );
        let filter = Some(WorkerAnyFilter {
            filters: vec![WorkerAllFilter {
                filters: vec![WorkerPropertyFilter::Status(WorkerStatusFilter {
                    comparator: FilterComparator::Equal,
                    value: WorkerStatus::Idle,
                })],
            }]
        });
        let mut workers: Vec<WorkerMetadata> = Vec::new();
        let getter = GetWorkers::new(template_id, filter.as_ref(), true);
        loop {
            match getter.get_next() {
                Some(values) => {
                    workers.extend(values);
                }
                None => break,
            }
        }
        workers
    }
}