//! Topo-sort (Kahn) sul grafo after ∪ requires, ristretto al set di nodi
//! passato. Dipendenze verso nomi fuori dal set sono ignorate (es. builtin
//! già attivi). Ritorna (ordine, nodi_in_ciclo).
use alloc::string::String;
use alloc::vec::Vec;

pub fn topo_sort(nodes: &[(String, Vec<String>)]) -> (Vec<String>, Vec<String>) {
    let in_set = |n: &str| nodes.iter().any(|(name, _)| name == n);
    // indegree = numero di dep DENTRO il set
    let mut indeg: Vec<usize> = nodes.iter()
        .map(|(_, deps)| deps.iter().filter(|d| in_set(d)).count())
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    let mut done = alloc::vec![false; nodes.len()];
    loop {
        // il più piccolo indice con indegree 0 non ancora emesso (deterministico)
        let Some(i) = (0..nodes.len()).find(|&i| !done[i] && indeg[i] == 0) else { break; };
        done[i] = true;
        order.push(nodes[i].0.clone());
        for (j, (_, deps)) in nodes.iter().enumerate() {
            if !done[j] && deps.iter().any(|d| *d == nodes[i].0) {
                indeg[j] = indeg[j].saturating_sub(1);
            }
        }
    }
    let cyclic = nodes.iter().enumerate()
        .filter(|(i, _)| !done[*i])
        .map(|(_, (n, _))| n.clone())
        .collect();
    (order, cyclic)
}
