import ctypes
import os
import sys


def publish(source, destination):
    source = os.path.abspath(source)
    destination = os.path.abspath(destination)
    if source == destination or os.path.commonpath([source, destination]) in (source, destination):
        raise ValueError('Release source and destination must be separate directories.')
    if not os.path.isdir(source) or os.path.islink(source):
        raise ValueError('The release bundle must be a real directory.')
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    if os.stat(source).st_dev != os.stat(os.path.dirname(destination)).st_dev:
        raise ValueError('Release publication requires one filesystem.')
    if os.path.lexists(destination):
        if not os.path.isdir(destination) or os.path.islink(destination):
            raise ValueError('The previous bundle must be a real directory.')
        library = ctypes.CDLL(None, use_errno=True)
        exchange = library.renameatx_np
        exchange.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        exchange.restype = ctypes.c_int
        if exchange(-2, os.fsencode(source), -2, os.fsencode(destination), 2) != 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code))
    else:
        os.rename(source, destination)
    for parent in {os.path.dirname(source), os.path.dirname(destination)}:
        descriptor = os.open(parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


if __name__ == '__main__':
    publish(*sys.argv[1:])
