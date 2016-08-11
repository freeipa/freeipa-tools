#!/usr/bin/python2
# inspired by: https://github.com/lmacken/fedmsg-koji-consumer

from setuptools import setup

setup(
    name='githubconsumer',
    version='0.0.1',
    description='',
    author='',
    author_email='',
    url='',
    install_requires=["fedmsg"],
    packages=[],
    entry_points="""
    [moksha.consumer]
    testgithubconsumer = githubconsumer:TestGithubConsumer
    sssdgithubconsumer = githubconsumer:SSSDGithubConsumer
    freeipagithubconsumer = githubconsumer:FreeIPAGithubConsumer
    """,
)
