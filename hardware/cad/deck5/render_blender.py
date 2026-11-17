"""render_blender.py — headless photoreal render of a deck GLB (from the
product-design skill's blender_render.py, plus camera controls).

  --az DEG --el DEG   camera azimuth (0 = in front of the deck, 90 = from +x) and elevation
  --dist K            camera distance in bbox radii (default 3.0)
  --target x,y,z      look at this point (metres, CAD frame) instead of the bbox centre
  --lens MM           focal length (default 50)
  --dark              dark studio (black backdrop, the screen glows)


Usage (Blender must be installed; on this machine: /opt/blender-5.0.1-linux-x64/blender):
  blender -b -P blender_render.py -- INPUT.glb OUTPUT.png [--samples 96] [--res 1100]
          [--frames N] [--bg] [--engine CYCLES|BLENDER_EEVEE_NEXT]

  --frames N   N-step 360deg TURNTABLE (object rotates under fixed lights/camera) ->
               OUTPUT_000.png, OUTPUT_001.png ...  (stitch to gif/mp4 with ffmpeg)
  --bg         solid studio backdrop instead of a transparent film (use this for turntables)
Builds a studio (world fill + area key/fill/rim, shadow-catcher floor), frames the camera to
the model bbox, AgX view transform. glTF Y-up imports to Blender Z-up automatically.
"""
import bpy
import sys
import math
from mathutils import Vector, Matrix


def argv():
    a = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    o = {"inp": a[0], "out": a[1], "samples": 96, "res": 1100,
         "frames": 0, "bg": False, "engine": "CYCLES", "az": 34.0, "el": 16.0,
         "dist": 3.0, "target": None, "lens": 50.0, "dark": False, "resy": None}
    i = 2
    while i < len(a):
        if a[i] == "--samples": o["samples"] = int(a[i + 1]); i += 2
        elif a[i] == "--res": o["res"] = int(a[i + 1]); i += 2
        elif a[i] == "--frames": o["frames"] = int(a[i + 1]); i += 2
        elif a[i] == "--engine": o["engine"] = a[i + 1]; i += 2
        elif a[i] == "--bg": o["bg"] = True; i += 1
        elif a[i] == "--dark": o["dark"] = True; o["bg"] = True; i += 1
        elif a[i] in ("--az", "--el", "--dist", "--lens"): o[a[i][2:]] = float(a[i + 1]); i += 2
        elif a[i] == "--resy": o["resy"] = int(a[i + 1]); i += 2
        elif a[i] == "--target": o["target"] = Vector([float(v) for v in a[i + 1].split(",")]); i += 2
        else: i += 1
    return o


def world_light(strength, color):
    w = bpy.context.scene.world or bpy.data.worlds.new("W")
    bpy.context.scene.world = w
    w.use_nodes = True
    bg = w.node_tree.nodes["Background"]
    bg.inputs[0].default_value = (*color, 1.0)
    bg.inputs[1].default_value = strength


def _aim(obj, target):
    obj.rotation_euler = (Vector(target) - obj.location).to_track_quat("-Z", "Y").to_euler()


def area(name, loc, energy, size, target):
    d = bpy.data.lights.new(name, "AREA"); d.energy = energy; d.size = size
    o = bpy.data.objects.new(name, d); o.location = loc
    bpy.context.collection.objects.link(o); _aim(o, target)


def bbox(objs):
    lo = Vector((1e9,) * 3); hi = Vector((-1e9,) * 3)
    for ob in objs:
        for c in ob.bound_box:
            w = ob.matrix_world @ Vector(c)
            lo = Vector(map(min, lo, w)); hi = Vector(map(max, hi, w))
    return lo, hi


def main():
    o = argv()
    bpy.ops.wm.read_factory_settings(use_empty=True)
    sc = bpy.context.scene

    bpy.ops.import_scene.gltf(filepath=o["inp"])
    model = [ob for ob in sc.objects if ob.type == "MESH"]
    lo, hi = bbox(model)
    center = (lo + hi) * 0.5
    radius = (hi - lo).length * 0.5

    # parent the model to an empty at its center so a turntable rotates the OBJECT
    turn = bpy.data.objects.new("turn", None); turn.location = center
    sc.collection.objects.link(turn)
    for ob in model:
        ob.parent = turn
        # explicit inverse (don't read turn.matrix_world — it isn't updated yet)
        ob.matrix_parent_inverse = Matrix.Translation(-center)

    # studio lights (fixed) + shadow-catcher floor (fixed)
    if o["dark"]:
        world_light(0.25, (0.02, 0.02, 0.025))
    else:
        world_light(0.6, (0.5, 0.52, 0.55) if o["bg"] else (0.05, 0.05, 0.06))
    area("key", center + Vector((radius, -radius * 1.3, radius * 1.6)),
         radius * radius * 22, radius * 2.2, center)
    area("fill", center + Vector((-radius * 1.4, -radius, radius * 0.6)),
         radius * radius * 7, radius * 3, center)
    area("rim", center + Vector((0, radius * 1.5, radius * 1.2)),
         radius * radius * 14, radius * 1.5, center)
    bpy.ops.mesh.primitive_plane_add(size=radius * 30, location=(center.x, center.y, lo.z))
    bpy.context.active_object.is_shadow_catcher = True

    # camera framed to the bbox (fixed)
    cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
    sc.collection.objects.link(cam); sc.camera = cam
    az, el, dist = math.radians(o["az"]), math.radians(o["el"]), radius * o["dist"]
    tgt = o["target"] if o["target"] is not None else center
    cam.location = tgt + Vector((dist * math.cos(el) * math.sin(az),
                                 -dist * math.cos(el) * math.cos(az),
                                 dist * math.sin(el)))
    cam.data.lens = o["lens"]
    cam.data.clip_start = 0.001
    _aim(cam, tgt)

    # render settings
    sc.render.engine = o["engine"]
    if o["engine"] == "CYCLES":
        sc.cycles.samples = o["samples"]
        prefs = bpy.context.preferences.addons["cycles"].preferences
        chosen = None
        for backend in ("OPTIX", "CUDA", "HIP", "ONEAPI", "METAL"):
            prefs.compute_device_type = backend
            try:
                prefs.refresh_devices()          # NB: refresh_devices(), not get_devices()
            except Exception:
                continue
            if any(d.type == backend for d in prefs.devices):
                for d in prefs.devices:
                    d.use = (d.type == backend)  # enable the GPU(s) only, skip CPU
                chosen = backend
                break
        sc.cycles.device = "GPU" if chosen else "CPU"
        print("CYCLES device:", chosen or "CPU",
              [d.name for d in prefs.devices if d.use] if chosen else "")
    sc.render.resolution_x = o["res"]
    sc.render.resolution_y = o["resy"] or o["res"]
    sc.render.film_transparent = not o["bg"]
    try:
        sc.view_settings.view_transform = "AgX"
    except Exception:
        sc.view_settings.view_transform = "Filmic"

    if o["frames"] <= 1:
        sc.render.filepath = o["out"]
        bpy.ops.render.render(write_still=True)
        print("WROTE", o["out"])
    else:
        base = o["out"].rsplit(".", 1)[0]
        for i in range(o["frames"]):
            turn.rotation_euler[2] = 2 * math.pi * i / o["frames"]
            bpy.context.view_layer.update()
            sc.render.filepath = f"{base}_{i:03d}.png"
            bpy.ops.render.render(write_still=True)
            print("WROTE", sc.render.filepath)


main()
