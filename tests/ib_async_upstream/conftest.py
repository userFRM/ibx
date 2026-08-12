"""Run ib_async's own test suite against this engine.

ib_async is layered: `IB`, `Wrapper`, `Ticker` and `Trade` are transport
agnostic, and only `Client`/`Connection` know there is a socket to a gateway.
`ibx.ib_async.attach` replaces that layer, so their library runs unmodified.
Their suite is the strongest available statement of whether it does.

Their tests are not vendored. Point the run at a checkout of theirs:

    git clone https://github.com/ib-api-reloaded/ib_async /tmp/ib_async
    cp tests/ib_async_upstream/conftest.py /tmp/ib_async/tests/
    IB_USERNAME=… IB_PASSWORD=… pytest /tmp/ib_async/tests \\
        -o asyncio_mode=auto \\
        -o asyncio_default_fixture_loop_scope=session \\
        -o asyncio_default_test_loop_scope=session

Both loop scopes are needed. Their session-scoped connection fixture and their
tests must share one event loop, or the callbacks land on a loop that is not
running while the test waits on them.
"""
import os

import ib_async as ibi
import pytest_asyncio

import ibx.ib_async


@pytest_asyncio.fixture(scope="session", loop_scope="session")
async def ib():
    ib = ibx.ib_async.attach(
        ibi.IB(),
        username=os.environ["IB_USERNAME"],
        password=os.environ["IB_PASSWORD"],
    )
    await ib.connectAsync()
    yield ib
    ib.disconnect()
