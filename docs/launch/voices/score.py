"""Predicted naturalness (UTMOSv2 MOS) of every candidate recording."""
import json, sys
import utmosv2
model = utmosv2.create_model(pretrained=True, device="cpu")
res = model.predict(input_dir=sys.argv[1], num_repetitions=3, device="cpu")
json.dump(res, open(sys.argv[2], "w"), indent=1)
for r in sorted(res, key=lambda r: -r["predicted_mos"]):
    print(f'{r["predicted_mos"]:.3f}', r["file_path"].split("/")[-1])
