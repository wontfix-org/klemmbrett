#!/usr/bin/env python

import configparser as _config

undefined = object()

class Config(_config.RawConfigParser):

    def get(self, section, option, default = undefined):
        try:
            return _config.RawConfigParser.get(self, section, option)
        except (_config.NoSectionError, _config.NoOptionError):
            if default is not undefined:
                return default
            raise


