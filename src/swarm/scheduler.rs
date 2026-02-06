use crate::error::{NexusError, Result};
use crate::swarm::architect::Task;
use std::collections::{HashMap, HashSet, VecDeque};

/// A stage in the execution plan containing tasks that can run in parallel
#[derive(Debug, Clone)]
pub struct ExecutionStage {
    pub stage_number: usize,
    pub tasks: Vec<Task>,
}

/// Complete execution plan with staged tasks
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub stages: Vec<ExecutionStage>,
    pub total_tasks: usize,
    pub critical_path: Vec<String>,
}

/// Scheduler manages task execution order and parallelization
pub struct Scheduler {
    max_concurrent: usize,
}

impl Scheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
        }
    }

    /// Create an execution plan from a set of tasks
    pub fn create_plan(&self, tasks: &[Task]) -> Result<ExecutionPlan> {
        let graph = self.build_dependency_graph(tasks);
        let stages = self.compute_stages(tasks, &graph)?;
        let critical_path = self.compute_critical_path(tasks, &graph);

        Ok(ExecutionPlan {
            total_tasks: tasks.len(),
            stages,
            critical_path,
        })
    }

    /// Build a dependency graph from tasks
    fn build_dependency_graph(&self, tasks: &[Task]) -> HashMap<String, Vec<String>> {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();

        for task in tasks {
            // Store reverse dependencies (task -> things it blocks)
            graph.entry(task.id.clone()).or_default();

            for dep in &task.dependencies {
                graph.entry(dep.clone()).or_default().push(task.id.clone());
            }
        }

        graph
    }

    /// Compute execution stages using topological sort
    fn compute_stages(
        &self,
        tasks: &[Task],
        graph: &HashMap<String, Vec<String>>,
    ) -> Result<Vec<ExecutionStage>> {
        let task_map: HashMap<String, &Task> = tasks.iter().map(|t| (t.id.clone(), t)).collect();

        // Calculate in-degrees
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for task in tasks {
            in_degree.entry(task.id.clone()).or_insert(0);
            for _dep in &task.dependencies {
                *in_degree.entry(task.id.clone()).or_insert(0) += 1;
            }
        }

        // Find all starting tasks (no dependencies)
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut stages = Vec::new();
        let mut processed: HashSet<String> = HashSet::new();

        while !queue.is_empty() {
            // Collect all tasks ready for this stage
            let stage_tasks: Vec<Task> = queue
                .drain(..)
                .filter_map(|id| task_map.get(&id).map(|&t| t.clone()))
                .collect();

            if stage_tasks.is_empty() {
                break;
            }

            // Limit concurrent tasks per stage
            let stage = ExecutionStage {
                stage_number: stages.len() + 1,
                tasks: if stage_tasks.len() > self.max_concurrent {
                    // Split into multiple stages if too many parallel tasks
                    let mut remaining = stage_tasks;
                    let current: Vec<Task> = remaining.drain(..self.max_concurrent).collect();

                    // Put remaining back in queue for next iteration
                    for task in &remaining {
                        queue.push_back(task.id.clone());
                    }

                    current
                } else {
                    stage_tasks
                },
            };

            // Mark tasks as processed and update dependencies
            for task in &stage.tasks {
                processed.insert(task.id.clone());

                // Reduce in-degree for dependent tasks
                if let Some(dependents) = graph.get(&task.id) {
                    for dependent in dependents {
                        if let Some(deg) = in_degree.get_mut(dependent) {
                            *deg -= 1;
                            if *deg == 0 && !processed.contains(dependent) {
                                queue.push_back(dependent.clone());
                            }
                        }
                    }
                }
            }

            stages.push(stage);
        }

        // Check for unprocessed tasks (circular dependency)
        if processed.len() != tasks.len() {
            let unprocessed: Vec<String> = tasks
                .iter()
                .filter(|t| !processed.contains(&t.id))
                .map(|t| t.id.clone())
                .collect();

            return Err(NexusError::Configuration(format!(
                "Unable to schedule all tasks. Unprocessed: {:?}",
                unprocessed
            )));
        }

        Ok(stages)
    }

    /// Compute the critical path (longest path through dependencies)
    fn compute_critical_path(
        &self,
        tasks: &[Task],
        graph: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        let task_map: HashMap<String, &Task> = tasks.iter().map(|t| (t.id.clone(), t)).collect();

        // Calculate earliest completion times
        let mut earliest: HashMap<String, u32> = HashMap::new();
        let mut path: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize with zero
        for task in tasks {
            earliest.insert(task.id.clone(), task.estimated_effort);
            path.insert(task.id.clone(), vec![task.id.clone()]);
        }

        // Topological order processing
        let topo_order = self.topological_sort(tasks, graph);

        for task_id in topo_order {
            if let Some(_task) = task_map.get(&task_id) {
                // Check all tasks that depend on this one
                if let Some(dependents) = graph.get(&task_id) {
                    for dependent_id in dependents {
                        if let Some(dependent) = task_map.get(dependent_id) {
                            let current_earliest = earliest.get(&task_id).copied().unwrap_or(0);
                            let dependent_time = earliest
                                .get(dependent_id)
                                .copied()
                                .unwrap_or(dependent.estimated_effort);

                            let new_time = current_earliest + dependent.estimated_effort;

                            if new_time > dependent_time {
                                earliest.insert(dependent_id.clone(), new_time);

                                // Update path
                                let mut new_path = path.get(&task_id).cloned().unwrap_or_default();
                                new_path.push(dependent_id.clone());
                                path.insert(dependent_id.clone(), new_path);
                            }
                        }
                    }
                }
            }
        }

        // Find the task with maximum earliest time
        path.values()
            .max_by_key(|p| p.iter().filter_map(|id| earliest.get(id)).sum::<u32>())
            .cloned()
            .unwrap_or_default()
    }

    /// Perform topological sort
    fn topological_sort(
        &self,
        tasks: &[Task],
        _graph: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        // Calculate in-degrees
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for task in tasks {
            in_degree.entry(task.id.clone()).or_insert(0);
            for dep in &task.dependencies {
                in_degree.entry(task.id.clone()).or_insert(0);
                // Count dependencies that this task has
                if tasks.iter().any(|t| &t.id == dep) {
                    *in_degree.get_mut(&task.id).unwrap() += 1;
                }
            }
        }

        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut result = Vec::new();

        while let Some(current) = queue.pop_front() {
            result.push(current.clone());

            // Find tasks that depend on current
            for task in tasks {
                if task.dependencies.contains(&current) {
                    if let Some(deg) = in_degree.get_mut(&task.id) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(task.id.clone());
                        }
                    }
                }
            }
        }

        result
    }

    /// Get task dependencies in order they need to be satisfied
    pub fn get_dependency_order(&self, task: &Task, all_tasks: &[Task]) -> Vec<String> {
        let task_map: HashMap<String, &Task> =
            all_tasks.iter().map(|t| (t.id.clone(), t)).collect();

        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let _stack: Vec<String> = Vec::new();

        // DFS to collect all dependencies
        fn dfs(
            task_id: &str,
            task_map: &HashMap<String, &Task>,
            visited: &mut HashSet<String>,
            result: &mut Vec<String>,
        ) {
            if visited.contains(task_id) {
                return;
            }

            visited.insert(task_id.to_string());

            if let Some(task) = task_map.get(task_id) {
                for dep in &task.dependencies {
                    dfs(dep, task_map, visited, result);
                }
                result.push(task_id.to_string());
            }
        }

        // Collect all dependencies
        for dep in &task.dependencies {
            dfs(dep, &task_map, &mut visited, &mut result);
        }

        result
    }

    /// Check if a task is ready to execute (all dependencies satisfied)
    pub fn is_ready(&self, task: &Task, completed: &HashSet<String>) -> bool {
        task.dependencies.iter().all(|dep| completed.contains(dep))
    }

    /// Get ready tasks from a list
    pub fn get_ready_tasks<'a>(
        &self,
        tasks: &'a [Task],
        completed: &HashSet<String>,
    ) -> Vec<&'a Task> {
        tasks
            .iter()
            .filter(|t| self.is_ready(t, completed))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_task(id: &str, deps: Vec<&str>, effort: u32) -> Task {
        Task {
            id: id.to_string(),
            description: format!("Task {}", id),
            worker_type_hint: None,
            dependencies: deps.into_iter().map(|s| s.to_string()).collect(),
            estimated_effort: effort,
            context: String::new(),
            status: crate::swarm::architect::TaskStatus::Pending,
        }
    }

    #[test]
    fn test_simple_scheduling() {
        let tasks = vec![
            create_test_task("A", vec![], 10),
            create_test_task("B", vec!["A"], 15),
            create_test_task("C", vec!["A"], 20),
        ];

        let scheduler = Scheduler::new(2);
        let plan = scheduler.create_plan(&tasks).unwrap();

        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].tasks.len(), 1); // A
        assert_eq!(plan.stages[1].tasks.len(), 2); // B, C
    }

    #[test]
    fn test_critical_path() {
        let tasks = vec![
            create_test_task("A", vec![], 10),
            create_test_task("B", vec!["A"], 20),
            create_test_task("C", vec!["B"], 30),
            create_test_task("D", vec!["A"], 5),
        ];

        let scheduler = Scheduler::new(4);
        let plan = scheduler.create_plan(&tasks).unwrap();

        // Critical path should be A -> B -> C (60 units)
        assert!(plan.critical_path.contains(&"A".to_string()));
        assert!(plan.critical_path.contains(&"B".to_string()));
        assert!(plan.critical_path.contains(&"C".to_string()));
    }

    #[test]
    fn test_parallel_limit() {
        let tasks = vec![
            create_test_task("A", vec![], 10),
            create_test_task("B", vec![], 10),
            create_test_task("C", vec![], 10),
            create_test_task("D", vec![], 10),
        ];

        let scheduler = Scheduler::new(2);
        let plan = scheduler.create_plan(&tasks).unwrap();

        // With limit of 2, we should have 2 stages
        assert!(plan.stages.len() >= 2);
    }
}
