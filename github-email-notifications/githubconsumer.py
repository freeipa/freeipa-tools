#!/usr/bin/python2
# inspired by: https://github.com/lmacken/fedmsg-koji-consumer

import fedmsg.consumers

from pprint import pprint


class GithubConsumer(fedmsg.consumers.FedmsgConsumer):
    topic = 'org.fedoraproject.prod.github.*'
    config_key = 'githubconsumer'

    def __init__(self, *args, **kw):
        super(GithubConsumer, self).__init__(*args, **kw)

    def consume(self, msg):
        pprint(msg)
