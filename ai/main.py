import gi
import numpy as np
from ultralytics import YOLO

gi.require_version("Gst", "1.0")
gi.require_version("GstApp", "1.0")

from gi.repository import GLib, Gst

Gst.init([])

RTSP_URL = "rtsp://210.99.70.120:1935/live/cctv001.stream"
TARGET_VIDEO_ADDR = ("127.0.0.1", 25000)

model = YOLO("yolo11n.pt")

pipeline_str = f"""
rtspsrc location="{RTSP_URL}" protocols=tcp latency=100 ! rtph264depay ! h264parse ! tee name=t

t. ! queue ! h264parse name=tx_parser config-interval=1 ! rtph264pay pt=96 ssrc=1000 ! udpsink host={TARGET_VIDEO_ADDR[0]} port={TARGET_VIDEO_ADDR[1]} sync=false async=false
"""


def on_ai_sample(pad, info, user_data):
    buffer = info.get_buffer()
    if buffer is None:
        return Gst.PadProbeReturn.OK

    success, mapinfo = buffer.map(Gst.MapFlags.WRITE)

    data = mapinfo.data

    sei = make_sei(f'{{"pts":{buffer.pts}}}'.encode())

    newbuf = Gst.Buffer.new_allocate(None, len(sei)+len(data), None)

    newbuf.fill(0, sei)
    newbuf.fill(len(sei), data)

    newbuf.pts = buffer.pts
    newbuf.dts = buffer.dts
    newbuf.duration = buffer.duration

    buffer.unmap(mapinfo)

    info.set_buffer(newbuf)

    return Gst.PadProbeReturn.OK


def make_sei(payload: bytes):
    start_code = b"\x00\x00\x00\x01"

    nal_header = bytes([0x06])       # nal_unit_type = 6 (SEI)

    payload_type = b"\x05"           # user_data_unregistered

    payload_size = bytes([16 + len(payload)])

    uuid = bytes.fromhex(
        "11223344556677889900AABBCCDDEEFF"
    )

    rbsp_trailing = b"\x80"

    return (
        start_code
        + nal_header
        + payload_type
        + payload_size
        + uuid
        + payload
        + rbsp_trailing
    )

pipeline = Gst.parse_launch(pipeline_str)

tx_parser = pipeline.get_by_name("tx_parser")
pad = tx_parser.get_static_pad("src")

pad.add_probe(Gst.PadProbeType.BUFFER, on_ai_sample, None)

pipeline.set_state(Gst.State.PLAYING)

loop = GLib.MainLoop()
try:
    loop.run()
except KeyboardInterrupt:
    pass
finally:
    pipeline.set_state(Gst.State.NULL)
