#!/usr/bin/env python3
"""Export criterion.rs benchmark results to a consolidated JSON summary."""
import json, os, sys, glob

criterion_dir = os.path.join(os.path.dirname(__file__), '..', 'target', 'criterion')
output_path = sys.argv[1] if len(sys.argv) > 1 else 'result/summary.json'

results = {}
if os.path.isdir(criterion_dir):
    for est_file in glob.glob(os.path.join(criterion_dir, '**', 'new', 'estimates.json'), recursive=True):
        rel = os.path.relpath(est_file, criterion_dir)
        bench_name = rel.replace('/new/estimates.json', '')
        with open(est_file) as f:
            data = json.load(f)
        results[bench_name] = {
            'mean_ns': data.get('mean', {}).get('point_estimate'),
            'stddev_ns': data.get('stddev', {}).get('point_estimate'),
        }

output = {
    'timestamp': os.path.basename(output_path).split('-')[1] if '-' in os.path.basename(output_path) else 'unknown',
    'commit': os.path.basename(output_path).split('-')[2].replace('.json', '') if '-' in os.path.basename(output_path) else 'unknown',
    'benchmarks': results,
}

os.makedirs(os.path.dirname(output_path), exist_ok=True)
with open(output_path, 'w') as f:
    json.dump(output, f, indent=2)
print(f"Exported {len(results)} benchmarks to {output_path}")
