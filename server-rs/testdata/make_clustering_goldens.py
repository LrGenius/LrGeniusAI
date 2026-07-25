"""M5 golden fixtures: real sklearn AgglomerativeClustering + DBSCAN
output on synthetic 512-d unit vectors, for verifying the Rust port."""
import json
import numpy as np
from sklearn.cluster import AgglomerativeClustering, DBSCAN

rng = np.random.default_rng(7)

def make_group(center_seed, n, spread, dim=8):
    rng2 = np.random.default_rng(center_seed)
    center = rng2.standard_normal(dim)
    center /= np.linalg.norm(center)
    pts = center + rng.standard_normal((n, dim)) * spread
    pts /= np.linalg.norm(pts, axis=1, keepdims=True)
    return pts

# 3 tight groups + 2 singleton outliers, low-dim (8) so groups are clearly separable
g1 = make_group(1, 4, 0.05)
g2 = make_group(2, 3, 0.05)
g3 = make_group(3, 3, 0.05)
outliers = make_group(4, 2, 0.9)  # spread far apart, act as noise/singletons
X = np.vstack([g1, g2, g3, outliers]).astype(np.float32)
n = X.shape[0]
print("n =", n)

results = {}
for linkage in ("complete", "average"):
    for threshold in (0.3, 0.6):
        c = AgglomerativeClustering(n_clusters=None, distance_threshold=threshold, metric="euclidean", linkage=linkage)
        labels = c.fit_predict(X)
        results[f"agg_{linkage}_{threshold}"] = labels.tolist()

for eps in (0.3, 0.6):
    for min_samples in (2, 3):
        c = DBSCAN(eps=eps, min_samples=min_samples, metric="euclidean")
        labels = c.fit_predict(X)
        results[f"dbscan_{eps}_{min_samples}"] = labels.tolist()

out = {"embeddings": X.tolist(), "results": results}
dst = "/Users/bm/src/LrGeniusAI/server-rs/testdata/clustering_goldens.json"
json.dump(out, open(dst, "w"))
print("wrote", dst)
for k, v in results.items():
    print(k, v)
